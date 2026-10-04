//! `tdm-napi` — native Node.js binding for the TDM decision runtime, published
//! as the npm package `@typedecision/runtime`.
//!
//! Exports (see `index.d.ts` for the shipped TypeScript surface):
//!
//! - [`judge`]: parse a [`tdm_core::DecisionRequest`] from JSON and route it
//!   through the [`tdm_runtime::Runtime`] engine — provider registry, routing,
//!   exact-hash cache, retry + circuit breaker, and audit — returning the
//!   [`tdm_core::DecisionResult`] as JSON.
//! - [`health`]: probe the default provider and return its
//!   [`tdm_core::HealthReport`] as JSON.
//!
//! # Provider selection & routing
//!
//! Judgment calls are routed by the [`tdm_runtime::Runtime`] engine: an
//! explicit `opts.provider` must name a registered provider and becomes the
//! routing override (unregistered names — including keyless `jev` — are
//! rejected with [`Status::InvalidArg`]); otherwise the config default
//! (`[defaults].provider`, overridable via `TDM_PROVIDER`) routes the call.
//!
//! The engine is built once per process from the ambient config
//! (`TDM_CONFIG_DIR` / platform config dir + env overrides) and cached in a
//! [`OnceLock`]; initialization failures are **not** cached, so a later call
//! retries. Two embedder affordances keep bare installs working:
//!
//! - **mock fallback**: when the config still routes its default through the
//!   stock keyless `jev` entry (no key in env / `auth.toml` / legacy
//!   `TYPESAFE_API_KEY`), the deterministic `mock` becomes the default —
//!   matching the pre-runtime env fallback.
//! - **mock always routable**: a `mock` entry is registered unless the config
//!   defines (e.g. disables) it itself, so `{ provider: "mock" }` works
//!   everywhere.
//!
//! `opts.session { harness, sessionId }` tags the call's audit/cache rows;
//! each field (and the whole object) defaults to `"unknown"`. Cache hits are
//! served transparently — the M1 surface does not expose `from_cache`.
//!
//! `health` probes the config default provider (the same route [`judge`]
//! takes without an override). The [`tdm_runtime::Runtime`] API has no health
//! passthrough, so the probe talks to the provider directly, gated on the
//! runtime registry actually containing that name.
//!
//! # Threading
//!
//! All async work runs on one shared multi-thread tokio runtime
//! ([`RUNTIME`], lazily initialized with `OnceLock`) — never a per-call
//! runtime. The engine itself is `Sync` and shared across calls.
//!
//! # Error mapping
//!
//! - a request/options JSON payload that fails to parse, or an unknown
//!   provider name -> `napi::Error` with [`napi::Status::InvalidArg`]
//! - [`tdm_core::TdmError`] (provider failure after retry / circuit / cache
//!   handling) -> `napi::Error` with [`napi::Status::GenericFailure`], reason
//!   prefixed with the error class (`[client]` / `[server]` / `[network]` /
//!   `[quality]` / `[unsupported]`)
//! - config / engine initialization failures -> `napi::Error` with
//!   [`napi::Status::GenericFailure`]

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use napi::{Error, Result, Status};
use napi_derive::napi;
use serde::Deserialize;
use serde_json::Value;
use tdm_core::{DecisionProvider, DecisionRequest, TdmError};
use tdm_provider_jev::{JevConfig, JevProvider};
use tdm_provider_mock::MockProvider;
use tdm_runtime::{Runtime, SessionCtx, TdmConfig, api_key_env};

/// Shared tokio runtime backing every exported async function. One instance
/// for the process lifetime; building a runtime per call is a footgun
/// (unbounded thread growth, lost timers).
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// A handle to the shared runtime, initializing it on first use.
fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime must build with valid configuration")
    })
}

/// The shared engine plus the metadata `health` needs to reach its provider.
struct Engine {
    runtime: Runtime,
    /// Config default provider after the bare-install mock fallback — the
    /// route `judge` takes without an override and `health` probes.
    default_provider: String,
    /// Direct handle onto `default_provider`'s implementation for the
    /// `health` probe (the [`Runtime`] API has no health passthrough).
    probe: Arc<dyn DecisionProvider>,
}

/// The process-wide engine. Initialization failures are never cached: a
/// failed build leaves this empty and the next call retries.
static ENGINE: OnceLock<Engine> = OnceLock::new();

/// A handle to the shared engine, initializing it on first use.
fn engine() -> Result<&'static Engine> {
    if let Some(engine) = ENGINE.get() {
        return Ok(engine);
    }
    let built = build_engine()?;
    Ok(ENGINE.get_or_init(|| built))
}

/// Loads the ambient config and builds the [`Engine`]: applies the
/// bare-install mock fallback, ensures `mock` is routable, and opens the
/// runtime (registry + cache + audit store).
fn build_engine() -> Result<Engine> {
    let config = TdmConfig::load().map_err(|error| {
        Error::new(
            Status::GenericFailure,
            format!("failed to load the tdm config: {error}"),
        )
    })?;
    engine_from_config(config)
}

/// Builds an [`Engine`] from an explicit config, applying the embedder
/// affordances documented on the crate: the stock keyless-`jev` default
/// falls back to `mock`, and `mock` is registered unless the config says
/// otherwise.
fn engine_from_config(mut config: TdmConfig) -> Result<Engine> {
    if stock_keyless_jev_default(&config) {
        config.provider.entry("mock".to_owned()).or_default();
        config.defaults.provider = "mock".to_owned();
    }
    // The deterministic mock needs no credentials; embedded callers can
    // always route to it explicitly. A config-defined entry (including
    // `[provider.mock] enabled = false`) is preserved.
    config.provider.entry("mock".to_owned()).or_default();

    let default_provider = config.defaults.provider.clone();
    let probe = build_probe_provider(&config, &default_provider)?;
    let runtime = Runtime::from_config(config).map_err(|error| {
        Error::new(
            Status::GenericFailure,
            format!("failed to initialize the tdm runtime: {error}"),
        )
    })?;
    Ok(Engine {
        runtime,
        default_provider,
        probe,
    })
}

/// Whether the config still routes its default through the stock keyless
/// `jev` entry: default is `jev`, the entry is a plain enabled jev provider,
/// and no API key resolves for it. This is the bare-install state where the
/// pre-runtime binding fell back to the deterministic mock.
fn stock_keyless_jev_default(config: &TdmConfig) -> bool {
    if config.defaults.provider != "jev" {
        return false;
    }
    let Some(entry) = config.provider.get("jev") else {
        return false;
    };
    entry.provider_type == "jev"
        && entry.enabled != Some(false)
        && resolve_jev_key(config, "jev").is_none()
}

/// Mirrors the runtime's auth resolution for one jev entry:
/// `TDM_<NAME>_API_KEY` env -> `auth.toml` -> legacy `TYPESAFE_API_KEY`.
fn resolve_jev_key(config: &TdmConfig, name: &str) -> Option<String> {
    std::env::var(api_key_env(name))
        .ok()
        .filter(|key| !key.is_empty())
        .or_else(|| config.auth.get(name).filter(|key| !key.is_empty()).cloned())
        .or_else(|| {
            std::env::var(tdm_runtime::config::LEGACY_TYPESAFE_KEY_ENV)
                .ok()
                .filter(|key| !key.is_empty())
        })
}

/// Builds the direct provider handle `health` probes for the engine's
/// default route. Judgment calls go through the [`Runtime`] pipeline (cache /
/// retry / circuit / audit); the probe talks to the provider directly.
fn build_probe_provider(config: &TdmConfig, name: &str) -> Result<Arc<dyn DecisionProvider>> {
    let Some(entry) = config.provider.get(name) else {
        return Err(Error::new(
            Status::GenericFailure,
            format!("default provider {name:?} has no [provider.{name}] config entry"),
        ));
    };
    match entry.provider_type.as_str() {
        "mock" => Ok(Arc::new(MockProvider::new())),
        "jev" => {
            let Some(api_key) = resolve_jev_key(config, name) else {
                return Err(Error::new(
                    Status::GenericFailure,
                    format!("provider {name:?} has no API key (env, auth.toml, TYPESAFE_API_KEY)"),
                ));
            };
            // Mirror the runtime registry's entry mapping: config overrides
            // the provider crate's pinned endpoint / model / timeout.
            let defaults = JevConfig::default();
            Ok(Arc::new(JevProvider::new(JevConfig {
                api_key,
                endpoint: entry.endpoint.clone().unwrap_or(defaults.endpoint),
                model: entry.model.clone().unwrap_or(defaults.model),
                timeout: Duration::from_millis(entry.timeout_ms.unwrap_or(10_000).max(1)),
            })))
        }
        other => Err(Error::new(
            Status::GenericFailure,
            format!("default provider {name:?} has unknown type {other:?}"),
        )),
    }
}

/// Per-call options accepted from JS.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JudgeOptions {
    /// Explicit provider override: must name a registered provider
    /// (`"jev"` / `"mock"` on stock installs).
    #[serde(default)]
    provider: Option<String>,
    /// Harness + session identity tagging the call's audit/cache rows.
    #[serde(default)]
    session: Option<SessionOptions>,
}

/// JS wire shape of the optional session object (`camelCase` keys).
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionOptions {
    #[serde(default)]
    harness: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
}

/// Converts the optional JS session into a [`SessionCtx`]: an absent object
/// or field defaults to `"unknown"` so audit rows always carry an identity.
fn session_ctx(session: Option<SessionOptions>) -> SessionCtx {
    let session = session.unwrap_or_default();
    SessionCtx::new(
        session.harness.unwrap_or_else(|| "unknown".to_owned()),
        session.session_id.unwrap_or_else(|| "unknown".to_owned()),
    )
}

/// Validates `opts.provider` against the runtime registry and converts it
/// into a [`Runtime::judge`] override. Unregistered names are
/// [`Status::InvalidArg`] — including `jev` when no API key is resolvable.
fn resolve_provider_override(engine: &Engine, explicit: Option<&str>) -> Result<Option<String>> {
    let Some(name) = explicit else {
        return Ok(None);
    };
    if engine.runtime.providers().contains(&name) {
        return Ok(Some(name.to_owned()));
    }
    if name == "jev" {
        return Err(Error::new(
            Status::InvalidArg,
            "provider \"jev\" requested but no API key is set (TDM_JEV_API_KEY or \
             TYPESAFE_API_KEY)",
        ));
    }
    Err(Error::new(
        Status::InvalidArg,
        format!(
            "unknown provider {name:?}: registered providers are {:?} (configure \
             [provider.{name}] in the tdm config)",
            engine.runtime.providers()
        ),
    ))
}

/// Parses the optional JS options object.
fn parse_options(raw: Option<Value>) -> Result<JudgeOptions> {
    match raw {
        None => Ok(JudgeOptions::default()),
        Some(raw) => serde_json::from_value(raw).map_err(|error| {
            Error::new(
                Status::InvalidArg,
                format!("invalid judge options: {error}"),
            )
        }),
    }
}

/// Parses a JSON payload into a [`DecisionRequest`].
fn parse_request(raw: Value) -> Result<DecisionRequest> {
    serde_json::from_value(raw).map_err(|error| {
        Error::new(
            Status::InvalidArg,
            format!("invalid DecisionRequest: {error}"),
        )
    })
}

/// Maps the [`TdmError`] taxonomy onto `napi::Error`: always
/// [`Status::GenericFailure`], with the class as a prefix of the Display
/// string so JS callers can distinguish `[client]` / `[server]` / `[network]`
/// / `[quality]` / `[unsupported]`.
fn tdm_error_to_napi(error: TdmError) -> Error {
    let reason = match &error {
        TdmError::Network { .. } => format!("[network] {error}"),
        TdmError::Server { .. } => format!("[server] {error}"),
        TdmError::Client { .. } => format!("[client] {error}"),
        TdmError::Quality { .. } => format!("[quality] {error}"),
        TdmError::Unsupported { .. } => format!("[unsupported] {error}"),
    };
    Error::new(Status::GenericFailure, reason)
}

/// Judges one batch of questions through the TDM engine.
///
/// The request and options are plain JSON objects (wire shape per
/// `@typedecision/contract`); the resolved [`tdm_core::DecisionResult`] is
/// returned as JSON. Routing: `opts.provider` when given (must be
/// registered), else the config default. The engine's exact-hash cache may
/// serve the result without touching the provider — cache hits are
/// indistinguishable from fresh results on this surface (`from_cache` is
/// deliberately not exposed in M1). Every call appends an audit row tagged
/// with `opts.session` (defaulting to `"unknown"`).
///
/// # Errors
/// [`Status::InvalidArg`] for malformed input or an unknown provider;
/// [`Status::GenericFailure`] for provider/serialization failures (message
/// prefixed with the TDM error class).
#[napi]
pub async fn judge(req: Value, opts: Option<Value>) -> Result<Value> {
    let request = parse_request(req)?;
    let options = parse_options(opts)?;
    let engine = engine()?;
    let provider_override = resolve_provider_override(engine, options.provider.as_deref())?;
    let session = session_ctx(options.session);

    let judged = runtime()
        .spawn(async move {
            engine
                .runtime
                .judge(session, request, provider_override.as_deref())
                .await
        })
        .await
        .map_err(|error| {
            Error::new(
                Status::GenericFailure,
                format!("judge task failed to complete: {error}"),
            )
        })?
        .map_err(tdm_error_to_napi)?;

    serde_json::to_value(&judged.result).map_err(|error| {
        Error::new(
            Status::GenericFailure,
            format!("failed to serialize DecisionResult: {error}"),
        )
    })
}

/// Probes the default provider's liveness/connectivity.
///
/// The probed provider is the config default (`[defaults].provider` after
/// the bare-install mock fallback — the same route [`judge`] takes without
/// an override; `TDM_PROVIDER` re-points it). The [`tdm_runtime::Runtime`]
/// API has no health passthrough, so the probe calls the provider directly,
/// gated on the runtime registry containing that name. Returns the
/// [`tdm_core::HealthReport`] as JSON.
///
/// # Errors
/// [`Status::GenericFailure`] when the engine cannot be built or the probe
/// fails to complete.
#[napi]
pub async fn health() -> Result<Value> {
    let engine = engine()?;
    let name = engine.default_provider.clone();
    if !engine.runtime.providers().contains(&name.as_str()) {
        return Err(Error::new(
            Status::GenericFailure,
            format!("default provider {name:?} is not registered in the runtime"),
        ));
    }
    let provider = Arc::clone(&engine.probe);

    let report = runtime()
        .spawn(async move { provider.health().await })
        .await
        .map_err(|error| {
            Error::new(
                Status::GenericFailure,
                format!("health task failed to complete: {error}"),
            )
        })?;

    serde_json::to_value(&report).map_err(|error| {
        Error::new(
            Status::GenericFailure,
            format!("failed to serialize HealthReport: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::Mutex;

    use tdm_core::{Primitive, Question};
    use tdm_runtime::ProviderConfig;
    use tempfile::TempDir;

    use super::*;

    // -- Environment handling ------------------------------------------------

    /// Serializes env mutation across tests in this binary.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Saves the listed variables and applies `Some(value)` / `None` (unset);
    /// restores everything on drop. Acquire [`ENV_LOCK`] before constructing.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvGuard {
        fn new(vars: &[(&'static str, Option<&str>)]) -> Self {
            let saved: Vec<(&'static str, Option<OsString>)> = vars
                .iter()
                .map(|(name, _)| (*name, std::env::var_os(name)))
                .collect();
            for (name, value) in vars {
                // SAFETY: env access is serialized process-wide via ENV_LOCK
                // and every test here runs single-threaded (#[tokio::test]
                // defaults to the current-thread runtime; plain #[test]s
                // share one thread pool worker at a time under the lock).
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.saved {
                // SAFETY: see EnvGuard::new.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    // -- Fixtures ------------------------------------------------------------

    /// A mock-default engine config with the audit db inside `dir`.
    fn mock_config(dir: &TempDir) -> TdmConfig {
        let mut config = TdmConfig::default();
        config.provider.clear();
        config
            .provider
            .insert("mock".to_owned(), ProviderConfig::default());
        config.defaults.provider = "mock".to_owned();
        config.audit.path = dir.path().join("audit.db");
        config
    }

    /// Builds an [`Engine`] from `config`, redirecting the audit db into a
    /// fresh tempdir so tests never touch the real home directory.
    fn test_engine(mut config: TdmConfig) -> Engine {
        let dir = TempDir::new().unwrap();
        config.audit.path = dir.path().join("audit.db");
        engine_from_config(config).expect("engine builds")
    }

    fn noul_request() -> DecisionRequest {
        DecisionRequest {
            state: serde_json::json!({ "files_changed": 3 }),
            questions: vec![Question {
                id: "q1".to_owned(),
                instructions: "is the change safe?".to_owned(),
                primitive: Primitive::Noul,
            }],
        }
    }

    // -- Selection / routing -------------------------------------------------

    #[test]
    fn explicit_mock_override_is_accepted() {
        let dir = TempDir::new().unwrap();
        let engine = test_engine(mock_config(&dir));
        assert_eq!(
            resolve_provider_override(&engine, Some("mock")).unwrap(),
            Some("mock".to_owned())
        );
    }

    #[test]
    fn explicit_jev_requires_a_key() {
        let _env = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("TDM_JEV_API_KEY", None),
            ("TYPESAFE_API_KEY", None),
            ("TDM_PROVIDER", None),
        ]);
        let dir = TempDir::new().unwrap();
        let mut config = TdmConfig::default();
        config.audit.path = dir.path().join("audit.db");
        // No key anywhere -> stock keyless jev default falls back to mock,
        // leaving `jev` unregistered and the override rejected.
        let engine = test_engine(config);
        assert_eq!(engine.default_provider, "mock");
        let error = resolve_provider_override(&engine, Some("jev")).unwrap_err();
        assert_eq!(error.status, Status::InvalidArg);
        assert!(error.reason.contains("TDM_JEV_API_KEY"));
    }

    #[test]
    fn key_present_keeps_jev_default_and_registers_jev() {
        let _env = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("TDM_JEV_API_KEY", Some("env-secret")),
            ("TYPESAFE_API_KEY", None),
            ("TDM_PROVIDER", None),
        ]);
        let engine = test_engine(TdmConfig::default());
        assert_eq!(engine.default_provider, "jev");
        assert_eq!(engine.runtime.providers(), vec!["jev", "mock"]);
        assert_eq!(
            resolve_provider_override(&engine, Some("jev")).unwrap(),
            Some("jev".to_owned())
        );
    }

    #[test]
    fn keyless_bare_install_falls_back_to_mock_default() {
        let _env = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("TDM_JEV_API_KEY", None),
            ("TYPESAFE_API_KEY", None),
            ("TDM_PROVIDER", None),
        ]);
        let dir = TempDir::new().unwrap();
        let mut config = TdmConfig::default();
        config.audit.path = dir.path().join("audit.db");
        let engine = test_engine(config);
        assert_eq!(engine.default_provider, "mock");
        assert_eq!(engine.runtime.providers(), vec!["mock"]);
    }

    #[test]
    fn unknown_provider_name_is_rejected() {
        let dir = TempDir::new().unwrap();
        let engine = test_engine(mock_config(&dir));
        let error = resolve_provider_override(&engine, Some("claude")).unwrap_err();
        assert_eq!(error.status, Status::InvalidArg);
        assert!(error.reason.contains("claude"));
    }

    // -- Session context -----------------------------------------------------

    #[test]
    fn session_ctx_defaults_to_unknown() {
        let session = session_ctx(None);
        assert_eq!(session.harness, "unknown");
        assert_eq!(session.session_id, "unknown");

        let session = session_ctx(Some(SessionOptions::default()));
        assert_eq!(session.harness, "unknown");
        assert_eq!(session.session_id, "unknown");
    }

    // -- Error mapping / parsing ---------------------------------------------

    #[test]
    fn error_prefix_follows_taxonomy() {
        let cases = [
            (
                TdmError::Network {
                    message: "down".into(),
                },
                "[network] network error: down",
            ),
            (
                TdmError::Server {
                    status: 503,
                    message: "outage".into(),
                },
                "[server] server error (status 503): outage",
            ),
            (
                TdmError::Client {
                    status: 400,
                    code: None,
                    message: "bad".into(),
                },
                "[client] client error (status 400, code None): bad",
            ),
            (
                TdmError::Quality {
                    detail: "junk".into(),
                },
                "[quality] quality failure: junk",
            ),
            (
                TdmError::Unsupported {
                    primitive: tdm_core::PrimitiveKind::Score,
                },
                "[unsupported] unsupported primitive: Score",
            ),
        ];
        for (error, expected) in cases {
            let mapped = tdm_error_to_napi(error);
            assert_eq!(mapped.status, Status::GenericFailure);
            assert_eq!(mapped.reason, expected);
        }
    }

    #[test]
    fn request_parse_failures_surface_as_invalid_arg() {
        let error = parse_request(serde_json::json!({ "questions": "nope" })).unwrap_err();
        assert_eq!(error.status, Status::InvalidArg);
        assert!(error.reason.contains("invalid DecisionRequest"));
    }

    #[test]
    fn options_parse_and_ignore_unknown_fields() {
        let options =
            parse_options(Some(serde_json::json!({ "provider": "mock", "future": 1 }))).unwrap();
        assert_eq!(options.provider.as_deref(), Some("mock"));
        let options = parse_options(None).unwrap();
        assert_eq!(options.provider, None);
        assert_eq!(options.session, None);
        let error = parse_options(Some(serde_json::json!({ "provider": 3 }))).unwrap_err();
        assert_eq!(error.status, Status::InvalidArg);
    }

    #[test]
    fn options_session_parses_camel_case_fields() {
        let options = parse_options(Some(serde_json::json!({
            "provider": "mock",
            "session": { "harness": "opencode", "sessionId": "s-42" }
        })))
        .unwrap();
        let session = options.session.expect("session present");
        assert_eq!(session.harness.as_deref(), Some("opencode"));
        assert_eq!(session.session_id.as_deref(), Some("s-42"));
        let session = session_ctx(Some(session));
        assert_eq!(session.harness, "opencode");
        assert_eq!(session.session_id, "s-42");
    }

    // -- End-to-end through the engine ---------------------------------------

    #[test]
    fn judge_with_session_ctx_succeeds_via_runtime_with_mock_default() {
        let _env = ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        // Minimal config keeping the audit db inside the tempdir; everything
        // else stays stock (default jev, no key) so the mock fallback applies.
        let audit_path = dir.path().join("audit.db");
        std::fs::write(
            dir.path().join("config.toml"),
            // TOML literal string: no escape processing on Windows backslashes.
            format!("[audit]\npath = '{}'\n", audit_path.to_str().unwrap()),
        )
        .unwrap();
        let _guard = EnvGuard::new(&[
            ("TDM_CONFIG_DIR", Some(dir.path().to_str().unwrap())),
            ("TDM_JEV_API_KEY", None),
            ("TYPESAFE_API_KEY", None),
            ("TDM_PROVIDER", None),
        ]);

        let engine = build_engine().expect("engine builds with mock fallback");
        assert_eq!(engine.default_provider, "mock");
        assert_eq!(engine.runtime.providers(), vec!["mock"]);

        runtime().block_on(async {
            let first = engine
                .runtime
                .judge(SessionCtx::new("test", "session-1"), noul_request(), None)
                .await
                .expect("judges via the runtime");
            assert_eq!(first.provider, "mock");
            assert!(!first.from_cache);

            let second = engine
                .runtime
                .judge(SessionCtx::new("test", "session-1"), noul_request(), None)
                .await
                .expect("second identical call");
            assert_eq!(second.result, first.result);
            assert!(
                second.from_cache,
                "second identical call must be served from the cache"
            );
        });
    }
}
