//! `tdm-runtime` — the TDM engine crate (plan §5).
//!
//! [`Runtime`] wraps a provider registry plus the cross-cutting policy stack
//! every harness adapter shares:
//!
//! - **registry** — `[provider.*]` config entries become providers:
//!   `type = "jev"` builds a [`tdm_provider_jev::JevProvider`] (needs an API
//!   key: `TDM_<NAME>_API_KEY` → `auth.toml` → legacy `TYPESAFE_API_KEY`;
//!   without one the entry is excluded with a warning), `type = "mock"`
//!   builds a [`tdm_provider_mock::MockProvider`]; unknown types and
//!   `enabled = false` entries are skipped.
//! - **routing** — [`Runtime::judge`] picks `provider_override`, else
//!   `[defaults].provider`; an unregistered name is a `TdmError::Client`
//!   (code `unknown_provider`).
//! - **capability check** — every requested primitive must appear in the
//!   routed provider's capabilities, else `TdmError::Unsupported`.
//! - **exact-hash cache** — a SQLite table keyed by [`cache_key`] (provider,
//!   model, canonical request JSON); fresh hits are served without touching
//!   the provider (still audited, `cached = 1`).
//! - **retry** — `Network` errors back off exponentially (`backoff_ms *
//!   2^(n-1)`) up to `retry.network_max` attempts; `Server` errors up to
//!   `retry.server_max`; `Client` / `Quality` / `Unsupported` return
//!   immediately.
//! - **circuit breaker** — per provider, in-memory; `circuit_threshold`
//!   consecutive Network/Server outcomes open it and subsequent calls fail
//!   fast with `Server { 503, "circuit open" }` until the cooldown (60 s by
//!   default) passes and a single half-open probe is admitted.
//! - **audit** — every judgment call appends one SQLite row (success, cache
//!   hit, or final failure); [`Runtime::audit_recent`] and [`Runtime::stats`]
//!   query them for `tdmm logs` / `tdmm stats`.
//!
//! Config resolution implements the M1 slice of plan §6: built-in defaults →
//! global `config.toml` (from `$TDM_CONFIG_DIR` or the platform config dir) →
//! environment overrides (`TDM_PROVIDER`, key variables). Project-level
//! `.tdm/config.toml` layering is a documented TODO for M2.
//!
//! Storage note: audit/cache **write** failures during `judge` are logged and
//! never fail a judgment call; the same failures surface as errors on the
//! query APIs.
//!
//! # Example
//!
//! Build a mock-backed runtime over an in-memory audit store and judge one
//! request (second identical call would be a cache hit):
//!
//! ```
//! use tdm_core::{DecisionRequest, Primitive, Question};
//! use tdm_runtime::{ProviderConfig, Runtime, SessionCtx, TdmConfig};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # tokio::runtime::Runtime::new()?.block_on(async {
//! let mut config = TdmConfig::default();
//! config.provider.clear();
//! config.provider.insert(
//!     "mock".to_owned(),
//!     ProviderConfig {
//!         provider_type: "mock".to_owned(),
//!         ..ProviderConfig::default()
//!     },
//! );
//! config.defaults.provider = "mock".to_owned();
//! config.audit.path = ":memory:".into(); // keeps the example side-effect free
//!
//! let runtime = Runtime::from_config(config)?;
//! let request = DecisionRequest {
//!     state: serde_json::json!({ "files_changed": 3 }),
//!     questions: vec![Question {
//!         id: "q1".to_owned(),
//!         instructions: "is the change safe?".to_owned(),
//!         primitive: Primitive::Noul,
//!     }],
//! };
//! let judged = runtime
//!     .judge(SessionCtx::new("doctest", "session-1"), request, None)
//!     .await?;
//! assert_eq!(judged.provider, "mock");
//! assert!(!judged.from_cache);
//! # Ok::<(), tdm_runtime::RuntimeError>(())
//! # })?;
//! # Ok(())
//! # }
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tdm_core::{
    DecisionProvider, DecisionRequest, DecisionResult, Primitive, PrimitiveKind, TdmError,
};
use tdm_provider_jev::{JevConfig, JevProvider};
use tdm_provider_mock::MockProvider;

pub mod circuit;
pub mod config;
pub mod error;
pub mod store;

pub use config::{
    AuditConfig, CacheConfig, DefaultsConfig, ProviderConfig, RetryConfig, TdmConfig, api_key_env,
};
pub use error::RuntimeError;
pub use store::{AuditFilter, AuditRow, StatsRow, cache_key};

use circuit::Circuits;
use store::{AuditInsert, Store};

/// Model id reported by [`tdm_provider_mock::MockProvider`]; keep in sync
/// with that crate so cache keys match the provider's own metadata.
const MOCK_MODEL: &str = "mock-1";

/// Harness-side session identity attached to every audit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCtx {
    /// Which harness issued the call (e.g. `"opencode"`, `"tdmm"`).
    pub harness: String,
    /// Harness-side conversation identifier.
    pub session_id: String,
}

impl SessionCtx {
    /// Convenience constructor.
    #[must_use]
    pub fn new(harness: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            harness: harness.into(),
            session_id: session_id.into(),
        }
    }
}

/// A judgment outcome plus routing metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct Judged {
    /// The provider's (possibly cached) result.
    pub result: DecisionResult,
    /// Whether `result` was served from the exact-hash cache.
    pub from_cache: bool,
    /// Registry name of the provider that (was routed to and) produced it.
    pub provider: String,
}

struct RegistryEntry {
    provider: Arc<dyn DecisionProvider>,
    /// Model id used in cache keys; known for config-built providers.
    model: Option<String>,
}

/// The TDM engine: registry + routing + cache + retry/circuit + audit.
///
/// Cheap to share; all judgment state lives behind interior mutability and
/// every public method takes `&self`.
pub struct Runtime {
    config: TdmConfig,
    registry: BTreeMap<String, RegistryEntry>,
    store: Store,
    circuits: Circuits,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("default_provider", &self.config.defaults.provider)
            .field("providers", &self.providers())
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// Builds a runtime from an explicit [`TdmConfig`]: constructs providers,
    /// verifies the default route exists, and opens the audit/cache database.
    ///
    /// # Errors
    /// [`RuntimeError::Other`] when `[defaults].provider` is not registered
    /// (unknown type, disabled, or missing API key), [`RuntimeError::Io`] /
    /// [`RuntimeError::Db`] when the audit database cannot be opened.
    pub fn from_config(config: TdmConfig) -> Result<Self, RuntimeError> {
        let registry = build_registry(&config);
        let default_name = config.defaults.provider.as_str();
        if !registry.contains_key(default_name) {
            return Err(RuntimeError::other(format!(
                "default provider {default_name:?} is not registered (available: {:?}); \
                 check [provider.{default_name}] (type / enabled / API key) and \
                 [defaults].provider",
                registry.keys().collect::<Vec<_>>()
            )));
        }
        let store = Store::open(&config::expand_home(&config.audit.path))?;
        Ok(Self {
            config,
            registry,
            store,
            circuits: Circuits::default(),
        })
    }

    /// Loads config via [`TdmConfig::load`] and builds the runtime.
    ///
    /// # Errors
    /// Everything [`TdmConfig::load`] and [`Runtime::from_config`] can raise.
    pub fn from_env() -> Result<Self, RuntimeError> {
        Self::from_config(TdmConfig::load()?)
    }

    /// Sorted registry names — the identifiers `judge`'s override and
    /// `[defaults].provider` refer to.
    #[must_use]
    pub fn providers(&self) -> Vec<&str> {
        self.registry.keys().map(String::as_str).collect()
    }

    /// Registers (or replaces) a provider under `name`, keeping the
    /// runtime's config-driven cache / retry / circuit / audit policy.
    ///
    /// Primarily for tests and embedders that wire providers which have no
    /// config-driven constructor; such entries carry no known model id, so
    /// their cache keys are derived from the name alone.
    pub fn register_provider(&mut self, name: &str, provider: Arc<dyn DecisionProvider>) {
        self.registry.insert(
            name.to_owned(),
            RegistryEntry {
                provider,
                model: None,
            },
        );
    }

    /// Judges one batch of questions.
    ///
    /// Routing: `provider_override` when given, else `[defaults].provider`.
    /// The full pipeline is capability check → cache → circuit breaker →
    /// retry loop → audit, and every call path (success, cache hit, or final
    /// failure) appends an audit row.
    ///
    /// # Errors
    /// [`TdmError`] per the retry / escalation taxonomy, including
    /// `Client` for unknown provider names and `Server { 503 }` while a
    /// circuit is open.
    pub async fn judge(
        &self,
        session: SessionCtx,
        req: DecisionRequest,
        provider_override: Option<&str>,
    ) -> Result<Judged, TdmError> {
        let started = Instant::now();
        let name = provider_override
            .unwrap_or(&self.config.defaults.provider)
            .to_owned();
        let request_json = serde_json::to_string(&req).unwrap_or_else(|_| "{}".to_owned());

        let Some(entry) = self.registry.get(&name) else {
            let error = TdmError::Client {
                status: 400,
                code: Some("unknown_provider".to_owned()),
                message: format!(
                    "provider {name:?} is not registered; available providers: {:?}",
                    self.providers()
                ),
            };
            self.audit_failure(AuditContext {
                session: &session,
                provider: &name,
                model: None,
                request_hash: &cache_key(&name, None, &req),
                request_json: &request_json,
                error: &error.to_string(),
                started,
            });
            return Err(error);
        };

        for question in &req.questions {
            let kind = primitive_kind(&question.primitive);
            if !entry.provider.capabilities().primitives.contains(&kind) {
                let error = TdmError::Unsupported { primitive: kind };
                self.audit_failure(AuditContext {
                    session: &session,
                    provider: &name,
                    model: entry.model.as_deref(),
                    request_hash: &cache_key(&name, entry.model.as_deref(), &req),
                    request_json: &request_json,
                    error: &error.to_string(),
                    started,
                });
                return Err(error);
            }
        }

        let request_hash = cache_key(&name, entry.model.as_deref(), &req);

        // Exact-hash cache: fresh hits skip the provider entirely.
        if self.config.cache.enabled {
            let ttl_secs = self.config.cache.ttl_hours.saturating_mul(3_600);
            match self.store.cache_get(&request_hash, ttl_secs, unix_now()) {
                Ok(Some(result)) => {
                    let answers_json =
                        serde_json::to_string(&result.answers).unwrap_or_else(|_| "[]".to_owned());
                    self.store
                        .audit_insert(
                            AuditInsert {
                                harness: &session.harness,
                                session_id: &session.session_id,
                                provider: &name,
                                model: result.provider.model.as_deref().or(entry.model.as_deref()),
                                request_hash: &request_hash,
                                request_json: &request_json,
                                answers_json: Some(&answers_json),
                                error: None,
                                latency_ms: elapsed_ms(started),
                                input_tokens: result.usage.input_tokens,
                                output_tokens: result.usage.output_tokens,
                                cached: true,
                            },
                            unix_now(),
                        )
                        .unwrap_or_else(|error| {
                            tracing::warn!(error = %error, "failed to append audit row");
                        });
                    return Ok(Judged {
                        result,
                        from_cache: true,
                        provider: name,
                    });
                }
                Ok(None) => {}
                Err(db_error) => {
                    tracing::warn!(
                        provider = %name,
                        error = %db_error,
                        "cache lookup failed; treating as a miss"
                    );
                }
            }
        }

        // Circuit breaker gate (per provider, in-memory).
        let threshold = self.config.retry.circuit_threshold.max(1);
        let cooldown = Duration::from_millis(self.config.retry.circuit_cooldown_ms);
        if let Err(circuit_error) = self.circuits.check(&name, cooldown) {
            self.audit_failure(AuditContext {
                session: &session,
                provider: &name,
                model: entry.model.as_deref(),
                request_hash: &request_hash,
                request_json: &request_json,
                error: &circuit_error.to_string(),
                started,
            });
            return Err(circuit_error);
        }

        let provider = Arc::clone(&entry.provider);
        let outcome = run_attempts(provider, req, &self.config.retry).await;

        match outcome {
            Ok(result) => {
                self.circuits.record_success(&name);
                if self.config.cache.enabled {
                    if let Err(db_error) =
                        self.store
                            .cache_put(&request_hash, &name, unix_now(), &result)
                    {
                        tracing::warn!(
                            provider = %name,
                            error = %db_error,
                            "cache store failed"
                        );
                    }
                }
                let answers_json =
                    serde_json::to_string(&result.answers).unwrap_or_else(|_| "[]".to_owned());
                self.store
                    .audit_insert(
                        AuditInsert {
                            harness: &session.harness,
                            session_id: &session.session_id,
                            provider: &name,
                            model: result.provider.model.as_deref().or(entry.model.as_deref()),
                            request_hash: &request_hash,
                            request_json: &request_json,
                            answers_json: Some(&answers_json),
                            error: None,
                            latency_ms: elapsed_ms(started),
                            input_tokens: result.usage.input_tokens,
                            output_tokens: result.usage.output_tokens,
                            cached: false,
                        },
                        unix_now(),
                    )
                    .unwrap_or_else(|error| {
                        tracing::warn!(error = %error, "failed to append audit row");
                    });
                Ok(Judged {
                    result,
                    from_cache: false,
                    provider: name,
                })
            }
            Err(error) => {
                match &error {
                    TdmError::Network { .. } | TdmError::Server { .. } => {
                        self.circuits.record_failure(&name, threshold);
                    }
                    _ => self.circuits.record_non_retryable(&name),
                }
                self.audit_failure(AuditContext {
                    session: &session,
                    provider: &name,
                    model: entry.model.as_deref(),
                    request_hash: &request_hash,
                    request_json: &request_json,
                    error: &error.to_string(),
                    started,
                });
                Err(error)
            }
        }
    }

    /// Most recent audit rows matching `filter`, newest first.
    ///
    /// # Errors
    /// [`RuntimeError::Db`] on SQLite failure — query APIs surface storage
    /// errors (unlike `judge`, which only logs them).
    pub fn audit_recent(
        &self,
        filter: AuditFilter,
        limit: u32,
    ) -> Result<Vec<AuditRow>, RuntimeError> {
        self.store.audit_recent(filter, limit)
    }

    /// Aggregates the audit log per (`provider`, `harness`).
    ///
    /// # Errors
    /// [`RuntimeError::Db`] on SQLite failure.
    pub fn stats(&self) -> Result<Vec<StatsRow>, RuntimeError> {
        self.store.stats()
    }

    /// Appends a failure audit row; storage errors are logged, not raised.
    fn audit_failure(&self, context: AuditContext<'_>) {
        self.store
            .audit_insert(
                AuditInsert {
                    harness: &context.session.harness,
                    session_id: &context.session.session_id,
                    provider: context.provider,
                    model: context.model,
                    request_hash: context.request_hash,
                    request_json: context.request_json,
                    answers_json: None,
                    error: Some(context.error),
                    latency_ms: elapsed_ms(context.started),
                    input_tokens: 0,
                    output_tokens: 0,
                    cached: false,
                },
                unix_now(),
            )
            .unwrap_or_else(|error| tracing::warn!(error = %error, "failed to append audit row"));
    }
}

/// Failure-path audit fields collected before the write.
struct AuditContext<'a> {
    session: &'a SessionCtx,
    provider: &'a str,
    model: Option<&'a str>,
    request_hash: &'a str,
    request_json: &'a str,
    error: &'a str,
    started: Instant,
}

/// Constructs the provider registry from `[provider.*]` entries: unknown
/// types and disabled entries are skipped, keyless jev entries are excluded
/// with a warning.
fn build_registry(config: &TdmConfig) -> BTreeMap<String, RegistryEntry> {
    let mut registry = BTreeMap::new();
    for (name, provider_config) in &config.provider {
        if provider_config.enabled == Some(false) {
            tracing::debug!(provider = %name, "provider disabled by config; skipping");
            continue;
        }
        match provider_config.provider_type.as_str() {
            "jev" => {
                let Some(api_key) = config::resolve_api_key(config, name, "jev") else {
                    tracing::warn!(
                        provider = %name,
                        env_var = %config::api_key_env(name),
                        "no API key for jev provider (env, auth.toml, TYPESAFE_API_KEY); \
                         excluding from registry"
                    );
                    continue;
                };
                // `JevConfig::default` carries the provider crate's pinned
                // endpoint / model / timeout; config entries override per key.
                let jev_config = JevConfig {
                    api_key,
                    endpoint: provider_config
                        .endpoint
                        .clone()
                        .unwrap_or_else(|| JevConfig::default().endpoint),
                    model: provider_config
                        .model
                        .clone()
                        .unwrap_or_else(|| JevConfig::default().model),
                    timeout: Duration::from_millis(
                        provider_config.timeout_ms.unwrap_or(10_000).max(1),
                    ),
                };
                let model = jev_config.model.clone();
                registry.insert(
                    name.clone(),
                    RegistryEntry {
                        provider: Arc::new(JevProvider::new(jev_config)),
                        model: Some(model),
                    },
                );
            }
            "mock" => {
                registry.insert(
                    name.clone(),
                    RegistryEntry {
                        provider: Arc::new(MockProvider::new()),
                        model: Some(MOCK_MODEL.to_owned()),
                    },
                );
            }
            other => {
                tracing::warn!(
                    provider = %name,
                    ty = %other,
                    "unknown provider type; skipping"
                );
            }
        }
    }
    registry
}

/// The retry loop: `Network` errors retry up to `network_max` total attempts,
/// `Server` errors up to `server_max`, everything else returns immediately.
/// Delays are deterministic: `backoff_ms * 2^(n-1)` before attempt `n + 1`.
async fn run_attempts(
    provider: Arc<dyn DecisionProvider>,
    req: DecisionRequest,
    retry: &RetryConfig,
) -> Result<DecisionResult, TdmError> {
    let mut attempts: u32 = 0;
    loop {
        attempts += 1;
        match provider.judge(req.clone()).await {
            Ok(result) => return Ok(result),
            Err(error) => {
                let budget = match &error {
                    TdmError::Network { .. } => retry.network_max,
                    TdmError::Server { .. } => retry.server_max,
                    _ => return Err(error),
                };
                if attempts < budget.max(1) {
                    tokio::time::sleep(backoff_delay(retry.backoff_ms, attempts)).await;
                    continue;
                }
                return Err(error);
            }
        }
    }
}

/// Backoff before the retry following `attempts`-th failed attempt.
fn backoff_delay(base_ms: u64, attempts: u32) -> Duration {
    let shift = attempts.saturating_sub(1).min(16);
    Duration::from_millis(base_ms.saturating_mul(1u64 << shift))
}

/// Coarse kind of a [`Primitive`] for capability negotiation.
fn primitive_kind(primitive: &Primitive) -> PrimitiveKind {
    match primitive {
        Primitive::Choice { .. } => PrimitiveKind::Choice,
        Primitive::Noul => PrimitiveKind::Noul,
        Primitive::Score { .. } => PrimitiveKind::Score,
    }
}

/// Current time as unix seconds (saturating at the epoch on clock skew).
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Milliseconds elapsed since `started`.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_saturates() {
        assert_eq!(backoff_delay(100, 1), Duration::from_millis(100));
        assert_eq!(backoff_delay(100, 2), Duration::from_millis(200));
        assert_eq!(backoff_delay(100, 3), Duration::from_millis(400));
        assert_eq!(backoff_delay(u64::MAX, 40), Duration::from_millis(u64::MAX));
    }

    #[test]
    fn primitive_kind_maps_all_variants() {
        assert_eq!(primitive_kind(&Primitive::Noul), PrimitiveKind::Noul);
        assert_eq!(
            primitive_kind(&Primitive::Choice { options: vec![] }),
            PrimitiveKind::Choice
        );
        assert_eq!(
            primitive_kind(&Primitive::Score {
                levels: vec!["a".to_owned(), "b".to_owned()]
            }),
            PrimitiveKind::Score
        );
    }
}
