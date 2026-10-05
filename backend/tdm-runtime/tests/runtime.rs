//! `tdm-runtime` integration tests (plan §10.3: every `TdmError` class'
//! retry / circuit / surfacing behavior).
//!
//! No network: providers are the deterministic [`MockProvider`] plus
//! hand-rolled wrappers (counting, flaky, restricted-capability, static
//! error). Timing-sensitive knobs (`backoff_ms`, `circuit_cooldown_ms`) are
//! short-circuited via config; production defaults stay at 100 ms / 60 s.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tdm_core::{
    Answer, Capabilities, DecisionProvider, DecisionRequest, DecisionResult, HealthReport,
    Primitive, PrimitiveKind, Question, TdmError,
};
use tdm_runtime::{
    AuditFilter, ProviderConfig, Runtime, SessionCtx, TdmConfig, api_key_env, cache_key,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Environment handling
// ---------------------------------------------------------------------------

/// Serializes env mutation across tests in this binary.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Saves the listed variables and applies `Some(value)` / `None` (unset);
/// restores everything on drop. Acquire [`ENV_LOCK`] before constructing.
struct EnvGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn new(vars: &[(&'static str, Option<&str>)]) -> Self {
        let saved: Vec<(&'static str, Option<std::ffi::OsString>)> = vars
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in vars {
            // SAFETY: env access is serialized process-wide via ENV_LOCK and
            // every test here runs single-threaded (#[tokio::test] defaults
            // to the current-thread runtime; plain #[test]s share one thread
            // pool worker at a time under the same lock).
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

// ---------------------------------------------------------------------------
// Test providers
// ---------------------------------------------------------------------------

/// All three primitives, generous state budget.
fn all_caps() -> Capabilities {
    Capabilities {
        primitives: vec![
            PrimitiveKind::Choice,
            PrimitiveKind::Noul,
            PrimitiveKind::Score,
        ],
        batch: true,
        max_state_bytes: 1 << 20,
    }
}

/// Delegates to [`MockProvider`] but counts `judge` invocations.
struct CountingProvider {
    inner: tdm_provider_mock::MockProvider,
    calls: AtomicUsize,
}

impl CountingProvider {
    fn new() -> Self {
        Self {
            inner: tdm_provider_mock::MockProvider::new(),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DecisionProvider for CountingProvider {
    fn id(&self) -> &str {
        "counting"
    }

    fn capabilities(&self) -> &Capabilities {
        self.inner.capabilities()
    }

    async fn judge(&self, req: DecisionRequest) -> Result<DecisionResult, TdmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.judge(req).await
    }

    async fn health(&self) -> HealthReport {
        self.inner.health().await
    }
}

/// Fails the first `fail_first` calls with `error`, then delegates to a
/// deterministic mock.
struct FlakyProvider {
    fail_first: usize,
    error: TdmError,
    calls: AtomicUsize,
}

impl FlakyProvider {
    fn new(fail_first: usize, error: TdmError) -> Self {
        Self {
            fail_first,
            error,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DecisionProvider for FlakyProvider {
    fn id(&self) -> &str {
        "flaky"
    }

    fn capabilities(&self) -> &Capabilities {
        shared_caps()
    }

    async fn judge(&self, req: DecisionRequest) -> Result<DecisionResult, TdmError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call <= self.fail_first {
            return Err(self.error.clone());
        }
        tdm_provider_mock::MockProvider::new().judge(req).await
    }

    async fn health(&self) -> HealthReport {
        HealthReport::default()
    }
}

/// Process-wide static capabilities backing providers that need to hand out
/// a `&Capabilities` reference.
fn shared_caps() -> &'static Capabilities {
    use std::sync::OnceLock;
    static CAPS: OnceLock<Capabilities> = OnceLock::new();
    CAPS.get_or_init(all_caps)
}

/// Always returns `error`; counts invocations.
struct StaticErrorProvider {
    error: TdmError,
    calls: AtomicUsize,
}

impl StaticErrorProvider {
    fn new(error: TdmError) -> Self {
        Self {
            error,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DecisionProvider for StaticErrorProvider {
    fn id(&self) -> &str {
        "static-error"
    }

    fn capabilities(&self) -> &Capabilities {
        shared_caps()
    }

    async fn judge(&self, _req: DecisionRequest) -> Result<DecisionResult, TdmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.error.clone())
    }

    async fn health(&self) -> HealthReport {
        HealthReport::default()
    }
}

/// Advertises a restricted capability set; judging itself would delegate.
struct RestrictedCapsProvider {
    caps: Capabilities,
    inner: tdm_provider_mock::MockProvider,
    calls: AtomicUsize,
}

impl RestrictedCapsProvider {
    fn noul_only() -> Self {
        Self::with_caps(vec![PrimitiveKind::Noul], 1 << 20)
    }

    /// A provider whose `max_state_bytes` ceiling is `max_state_bytes`.
    fn tiny_state(max_state_bytes: usize) -> Self {
        Self::with_caps(vec![PrimitiveKind::Noul], max_state_bytes)
    }

    fn with_caps(primitives: Vec<PrimitiveKind>, max_state_bytes: usize) -> Self {
        Self {
            caps: Capabilities {
                primitives,
                batch: true,
                max_state_bytes,
            },
            inner: tdm_provider_mock::MockProvider::new(),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DecisionProvider for RestrictedCapsProvider {
    fn id(&self) -> &str {
        "restricted"
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    async fn judge(&self, req: DecisionRequest) -> Result<DecisionResult, TdmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.judge(req).await
    }

    async fn health(&self) -> HealthReport {
        self.inner.health().await
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A mock-default runtime config with the audit db inside `dir`.
fn mock_config(dir: &TempDir) -> TdmConfig {
    let mut config = TdmConfig::default();
    config.provider.clear();
    config.provider.insert(
        "mock".to_owned(),
        ProviderConfig {
            provider_type: "mock".to_owned(),
            ..ProviderConfig::default()
        },
    );
    config.defaults.provider = "mock".to_owned();
    config.audit.path = dir.path().join("audit.db");
    config
}

/// Builds the mock-default runtime, applying `tweak` first.
fn setup(dir: &TempDir, tweak: impl FnOnce(&mut TdmConfig)) -> Runtime {
    let mut config = mock_config(dir);
    tweak(&mut config);
    Runtime::from_config(config).expect("runtime builds")
}

fn noul_request(state: serde_json::Value) -> DecisionRequest {
    DecisionRequest {
        state,
        questions: vec![Question {
            id: "q1".to_owned(),
            instructions: "is the change safe?".to_owned(),
            primitive: Primitive::Noul,
        }],
    }
}

fn choice_request() -> DecisionRequest {
    DecisionRequest {
        state: serde_json::json!({ "danger": "high" }),
        questions: vec![Question {
            id: "q1".to_owned(),
            instructions: "pick a lane".to_owned(),
            primitive: Primitive::Choice {
                options: vec!["a".to_owned(), "b".to_owned()],
            },
        }],
    }
}

fn network_error() -> TdmError {
    TdmError::Network {
        message: "synthetic outage".to_owned(),
    }
}

/// Mirrors `tdmm init`'s skeleton (`backend/tdmm/src/config.rs::CONFIG_TOML`
/// is the source of truth; copied verbatim so schema drift is caught here).
const TDMM_CONFIG_SKELETON: &str = r#"# TDM configuration — created by `tdmm init`.
# Safe to version-control: no secrets live here (auth.toml is separate).
# Parsing and layered resolution (global -> project -> env -> args) land in M1;
# this skeleton exists so the layout is stable from day one.

version = 1

[provider.jev]
type = "jev"
endpoint = "https://api.typesafe.ai/v1/systemone"
timeout_ms = 10000
# pricing = { per_1k_input = 0.0, per_1k_output = 0.0 }   # optional, cost accounting

# Reserved placeholder for the StartLux-Decision provider until capability
# negotiation is complete. Uncommenting before then changes nothing.
# [provider.startlux]
# type = "startlux"
# enabled = false

[defaults]
provider = "jev"           # `tdmm use <provider>` rewrites this line (M1)
confidence_floor = 0.55    # adapter-side: below this, escalate to a human

[cache]
enabled = true
ttl_hours = 168

[retry]
network_max = 3            # exponential backoff for network errors
server_max = 2             # limited retries for 5xx
circuit_threshold = 5      # consecutive failures -> circuit opens

[audit]
path = "~/.local/share/tdm/audit.db"   # state lives in SQLite, never here
"#;

/// Mirrors `tdmm init`'s auth skeleton (`backend/tdmm/src/config.rs::AUTH_TOML`).
const TDMM_AUTH_SKELETON: &str = r#"# TDM auth secrets — NEVER commit or share this file.
#
# Prefer environment variables (what `tdmm call` reads today, and they will
# override this file once layered resolution lands in M1):
#   TDM_JEV_API_KEY   primary
#   TYPESAFE_API_KEY  fallback
#
# `tdmm keys set/get/list/rm` lands in M1; edit by hand until then.

[jev]
api_key = ""
"#;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[test]
fn config_defaults_when_files_missing() {
    let _env = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::new(&[("TDM_PROVIDER", None)]);
    let dir = TempDir::new().unwrap();

    let config = TdmConfig::load_from_dir(dir.path()).unwrap();

    assert_eq!(config.version, 1);
    assert_eq!(config.defaults.provider, "jev");
    assert!((config.defaults.confidence_floor - 0.55).abs() < f64::EPSILON);
    assert!(config.cache.enabled);
    assert_eq!(config.cache.ttl_hours, 168);
    assert_eq!(config.retry.network_max, 3);
    assert_eq!(config.retry.server_max, 2);
    assert_eq!(config.retry.circuit_threshold, 5);
    assert_eq!(config.retry.backoff_ms, 100);
    assert_eq!(config.retry.circuit_cooldown_ms, 60_000);
    assert_eq!(config.audit.path, Path::new("~/.local/share/tdm/audit.db"));
    assert!(config.auth.is_empty());

    let jev = &config.provider["jev"];
    assert_eq!(jev.provider_type, "jev");
    assert_eq!(
        jev.endpoint.as_deref(),
        Some("https://api.typesafe.ai/v1/systemone")
    );
    assert_eq!(jev.timeout_ms, Some(10_000));
}

#[test]
fn config_parses_tdmm_skeleton_shape() {
    let _env = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::new(&[("TDM_PROVIDER", None)]);
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("config.toml"), TDMM_CONFIG_SKELETON).unwrap();
    std::fs::write(dir.path().join("auth.toml"), TDMM_AUTH_SKELETON).unwrap();

    let config = TdmConfig::load_from_dir(dir.path()).unwrap();

    assert_eq!(config.version, 1);
    assert_eq!(config.defaults.provider, "jev");
    assert_eq!(
        config.provider["jev"].endpoint.as_deref(),
        Some("https://api.typesafe.ai/v1/systemone")
    );
    assert_eq!(config.provider["jev"].timeout_ms, Some(10_000));
    assert_eq!(config.cache.ttl_hours, 168);
    assert_eq!(config.retry.circuit_threshold, 5);
    // Skeleton auth key is empty -> not registered as a key.
    assert!(!config.auth.contains_key("jev"));

    // A filled-in auth.toml lands in `config.auth`.
    std::fs::write(
        dir.path().join("auth.toml"),
        "[jev]\napi_key = \"file-secret\"\n",
    )
    .unwrap();
    let config = TdmConfig::load_from_dir(dir.path()).unwrap();
    assert_eq!(
        config.auth.get("jev").map(String::as_str),
        Some("file-secret")
    );
}

#[test]
fn env_overrides_default_provider() {
    let _env = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::new(&[("TDM_PROVIDER", Some("mock"))]);
    let dir = TempDir::new().unwrap();

    let config = TdmConfig::load_from_dir(dir.path()).unwrap();
    assert_eq!(config.defaults.provider, "mock");
}

#[test]
fn api_key_env_maps_names() {
    assert_eq!(api_key_env("jev"), "TDM_JEV_API_KEY");
}

#[test]
fn from_env_builds_runtime_via_config_dir_and_key_env() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let _guard = EnvGuard::new(&[
        ("TDM_CONFIG_DIR", Some(dir.path().to_str().unwrap())),
        ("TDM_JEV_API_KEY", Some("env-secret")),
        ("TYPESAFE_API_KEY", None),
        ("TDM_PROVIDER", None),
    ]);

    let runtime = Runtime::from_env().expect("from_env builds");
    assert_eq!(runtime.providers(), vec!["jev"]);
}

// ---------------------------------------------------------------------------
// Auth resolution + registry construction
// ---------------------------------------------------------------------------

#[test]
fn jev_without_any_key_is_excluded_and_default_fails() {
    let _env = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::new(&[("TDM_JEV_API_KEY", None), ("TYPESAFE_API_KEY", None)]);
    let dir = TempDir::new().unwrap();

    let mut config = TdmConfig::default();
    config.audit.path = dir.path().join("audit.db");

    let error = Runtime::from_config(config).expect_err("jev needs a key");
    let message = error.to_string();
    assert!(message.contains("jev"), "clear error: {message}");
    assert!(message.contains("not registered"), "clear error: {message}");
}

#[test]
fn jev_key_from_auth_toml_registers_provider() {
    let _env = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::new(&[("TDM_JEV_API_KEY", None), ("TYPESAFE_API_KEY", None)]);
    let dir = TempDir::new().unwrap();

    let mut config = mock_config(&dir);
    config.provider.insert(
        "jev".to_owned(),
        ProviderConfig {
            provider_type: "jev".to_owned(),
            ..ProviderConfig::default()
        },
    );
    config
        .auth
        .insert("jev".to_owned(), "file-secret".to_owned());

    let runtime = Runtime::from_config(config).expect("auth.toml key registers jev");
    assert_eq!(runtime.providers(), vec!["jev", "mock"]);
}

#[test]
fn jev_env_key_overrides_auth_file() {
    let _env = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::new(&[
        ("TDM_JEV_API_KEY", Some("env-secret")),
        ("TYPESAFE_API_KEY", None),
    ]);

    let dir = TempDir::new().unwrap();
    let mut config = mock_config(&dir);
    config.provider.insert(
        "jev".to_owned(),
        ProviderConfig {
            provider_type: "jev".to_owned(),
            ..ProviderConfig::default()
        },
    );
    config
        .auth
        .insert("jev".to_owned(), "file-secret".to_owned());

    let runtime = Runtime::from_config(config).expect("env key wins");
    assert!(runtime.providers().contains(&"jev"));
}

#[test]
fn jev_legacy_typesafe_key_fallback() {
    let _env = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::new(&[
        ("TDM_JEV_API_KEY", None),
        ("TYPESAFE_API_KEY", Some("legacy")),
    ]);

    let dir = TempDir::new().unwrap();
    let mut config = mock_config(&dir);
    config.provider.insert(
        "jev".to_owned(),
        ProviderConfig {
            provider_type: "jev".to_owned(),
            ..ProviderConfig::default()
        },
    );

    let runtime = Runtime::from_config(config).expect("legacy key accepted");
    assert!(runtime.providers().contains(&"jev"));
}

#[test]
fn disabled_and_unknown_type_entries_are_skipped() {
    let dir = TempDir::new().unwrap();

    let mut config = mock_config(&dir);
    config.provider.insert(
        "off".to_owned(),
        ProviderConfig {
            provider_type: "mock".to_owned(),
            enabled: Some(false),
            ..ProviderConfig::default()
        },
    );
    config.provider.insert(
        "weird".to_owned(),
        ProviderConfig {
            provider_type: "mystery".to_owned(),
            ..ProviderConfig::default()
        },
    );

    let runtime = Runtime::from_config(config).expect("skips bad entries");
    assert_eq!(runtime.providers(), vec!["mock"]);
}

#[test]
fn memory_audit_path_opens_without_files() {
    let dir = TempDir::new().unwrap();
    let runtime = setup(&dir, |config| {
        config.audit.path = ":memory:".into();
    });
    assert!(
        runtime
            .audit_recent(AuditFilter::default(), 10)
            .unwrap()
            .is_empty()
    );
    assert!(runtime.stats().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Routing + capability check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn default_route_and_override() {
    let dir = TempDir::new().unwrap();
    let mut config = mock_config(&dir);
    config.provider.insert(
        "mock-b".to_owned(),
        ProviderConfig {
            provider_type: "mock".to_owned(),
            ..ProviderConfig::default()
        },
    );
    let runtime = Runtime::from_config(config).unwrap();
    assert_eq!(runtime.providers(), vec!["mock", "mock-b"]);

    let judged = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            None,
        )
        .await
        .unwrap();
    assert_eq!(judged.provider, "mock");

    let judged = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(2)),
            Some("mock-b"),
        )
        .await
        .unwrap();
    assert_eq!(judged.provider, "mock-b");
    assert!(!judged.from_cache);
}

#[tokio::test]
async fn unknown_provider_is_client_error_and_audited() {
    let dir = TempDir::new().unwrap();
    let runtime = setup(&dir, |_config| {});

    let error = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("nope"),
        )
        .await
        .unwrap_err();

    match error {
        TdmError::Client {
            status,
            code,
            message,
        } => {
            assert_eq!(status, 400);
            assert_eq!(code.as_deref(), Some("unknown_provider"));
            assert!(message.contains("nope"));
        }
        other => panic!("expected Client error, got {other:?}"),
    }

    let rows = runtime.audit_recent(AuditFilter::default(), 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].provider, "nope");
    assert!(
        rows[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("not registered")
    );
    assert!(!rows[0].cached);
    assert!(rows[0].answers_json.is_none());
}

#[tokio::test]
async fn unsupported_primitive_is_rejected_before_provider_call() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |_config| {});
    runtime.register_provider("noul-only", Arc::new(RestrictedCapsProvider::noul_only()));

    let error = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            choice_request(),
            Some("noul-only"),
        )
        .await
        .unwrap_err();

    assert_eq!(
        error,
        TdmError::Unsupported {
            primitive: PrimitiveKind::Choice
        }
    );

    let rows = runtime.audit_recent(AuditFilter::default(), 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("unsupported")
    );
}

#[tokio::test]
async fn oversized_state_is_client_413_without_provider_call() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |_config| {});
    let tiny = Arc::new(RestrictedCapsProvider::tiny_state(8));
    runtime.register_provider("tiny", Arc::clone(&tiny) as Arc<dyn DecisionProvider>);

    let error = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!("this state is way more than eight bytes")),
            Some("tiny"),
        )
        .await
        .unwrap_err();

    match error {
        TdmError::Client {
            status,
            code,
            message,
        } => {
            assert_eq!(status, 413);
            assert_eq!(code.as_deref(), Some("state_too_large"));
            assert!(
                message.contains("bytes"),
                "message carries sizes: {message}"
            );
        }
        other => panic!("expected Client 413, got {other:?}"),
    }
    assert_eq!(tiny.calls(), 0, "rejected before any provider call");

    // Pre-flight failures are audited like the others.
    let rows = runtime.audit_recent(AuditFilter::default(), 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("state_too_large")
    );
    assert!(rows[0].answers_json.is_none());

    // A state within budget flows through untouched.
    let judged = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("tiny"),
        )
        .await
        .expect("small state passes the size gate");
    assert!(!judged.from_cache);
    assert_eq!(tiny.calls(), 1);
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

#[tokio::test]
async fn second_identical_call_hits_cache() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |_config| {});
    let counting = Arc::new(CountingProvider::new());
    runtime.register_provider(
        "counting",
        Arc::clone(&counting) as Arc<dyn DecisionProvider>,
    );

    let request = noul_request(serde_json::json!({ "repo": "tdm" }));
    let session = SessionCtx::new("t", "cache-1");

    let first = runtime
        .judge(session.clone(), request.clone(), Some("counting"))
        .await
        .unwrap();
    assert!(!first.from_cache);
    assert_eq!(counting.calls(), 1);

    let second = runtime
        .judge(session.clone(), request.clone(), Some("counting"))
        .await
        .unwrap();
    assert!(second.from_cache);
    assert_eq!(counting.calls(), 1, "provider not invoked on cache hit");
    assert_eq!(
        first.result, second.result,
        "usage/latency identical from cache"
    );

    // Audit: newest row is the cache hit.
    let rows = runtime.audit_recent(AuditFilter::default(), 10).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].cached);
    assert!(!rows[1].cached);
    let expected_hash = cache_key("counting", None, &request);
    assert_eq!(rows[0].request_hash, expected_hash);
    assert_eq!(rows[1].request_hash, expected_hash);
    assert!(rows[0].error.is_none());
    let answers: Vec<Answer> =
        serde_json::from_str(rows[0].answers_json.as_deref().unwrap()).unwrap();
    assert_eq!(answers, second.result.answers);
}

#[tokio::test]
async fn different_state_is_a_cache_miss() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |_config| {});
    let counting = Arc::new(CountingProvider::new());
    runtime.register_provider(
        "counting",
        Arc::clone(&counting) as Arc<dyn DecisionProvider>,
    );

    runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("counting"),
        )
        .await
        .unwrap();
    runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(2)),
            Some("counting"),
        )
        .await
        .unwrap();

    assert_eq!(counting.calls(), 2, "different state must not hit");
}

#[tokio::test]
async fn expired_ttl_refetches() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |config| {
        config.cache.ttl_hours = 0; // entries are stale immediately
    });
    let counting = Arc::new(CountingProvider::new());
    runtime.register_provider(
        "counting",
        Arc::clone(&counting) as Arc<dyn DecisionProvider>,
    );

    let request = noul_request(serde_json::json!(1));
    let first = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            request.clone(),
            Some("counting"),
        )
        .await
        .unwrap();
    let second = runtime
        .judge(SessionCtx::new("t", "s1"), request, Some("counting"))
        .await
        .unwrap();

    assert!(!first.from_cache);
    assert!(!second.from_cache);
    assert_eq!(counting.calls(), 2);
}

#[tokio::test]
async fn disabled_cache_never_hits() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |config| {
        config.cache.enabled = false;
    });
    let counting = Arc::new(CountingProvider::new());
    runtime.register_provider(
        "counting",
        Arc::clone(&counting) as Arc<dyn DecisionProvider>,
    );

    let request = noul_request(serde_json::json!(1));
    let first = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            request.clone(),
            Some("counting"),
        )
        .await
        .unwrap();
    let second = runtime
        .judge(SessionCtx::new("t", "s1"), request, Some("counting"))
        .await
        .unwrap();

    assert!(!first.from_cache);
    assert!(!second.from_cache);
    assert_eq!(counting.calls(), 2);
}

#[tokio::test]
async fn cache_clear_forces_a_refetch() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |_config| {});
    let counting = Arc::new(CountingProvider::new());
    runtime.register_provider(
        "counting",
        Arc::clone(&counting) as Arc<dyn DecisionProvider>,
    );

    let request = noul_request(serde_json::json!(1));
    let session = SessionCtx::new("t", "s1");
    runtime
        .judge(session.clone(), request.clone(), Some("counting"))
        .await
        .unwrap();
    let hit = runtime
        .judge(session.clone(), request.clone(), Some("counting"))
        .await
        .unwrap();
    assert!(hit.from_cache);

    // Scoped clear does not touch other providers; clear-all evicts.
    assert_eq!(runtime.cache_clear(Some("nope")).unwrap(), 0);
    assert_eq!(
        runtime.cache_clear(Some("counting")).unwrap(),
        1,
        "the one cached row is removed"
    );

    let refetched = runtime
        .judge(session, request, Some("counting"))
        .await
        .unwrap();
    assert!(!refetched.from_cache, "cleared entry must be refetched");
    assert_eq!(counting.calls(), 2);
}

// ---------------------------------------------------------------------------
// Retry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn network_errors_retry_until_recovery() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |config| {
        config.retry.network_max = 3;
        config.retry.backoff_ms = 1;
    });
    let flaky = Arc::new(FlakyProvider::new(2, network_error()));
    runtime.register_provider("flaky", Arc::clone(&flaky) as Arc<dyn DecisionProvider>);

    let judged = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("flaky"),
        )
        .await
        .expect("recovers on attempt 3");

    assert!(!judged.from_cache);
    assert_eq!(flaky.calls(), 3);

    let rows = runtime.audit_recent(AuditFilter::default(), 10).unwrap();
    assert_eq!(rows.len(), 1, "one audit row for the whole call");
    assert!(rows[0].error.is_none());
    assert!(!rows[0].cached);
}

#[tokio::test]
async fn network_errors_exhaust_budget() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |config| {
        config.retry.network_max = 2;
        config.retry.backoff_ms = 1;
    });
    let broken = Arc::new(StaticErrorProvider::new(network_error()));
    runtime.register_provider("broken", Arc::clone(&broken) as Arc<dyn DecisionProvider>);

    let error = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("broken"),
        )
        .await
        .unwrap_err();

    assert_eq!(error, network_error());
    assert_eq!(broken.calls(), 2, "network_max total attempts");
}

#[tokio::test]
async fn server_errors_retry_up_to_server_max() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |config| {
        config.retry.network_max = 5;
        config.retry.server_max = 2;
        config.retry.backoff_ms = 1;
    });
    let broken = Arc::new(StaticErrorProvider::new(TdmError::Server {
        status: 500,
        message: "boom".to_owned(),
    }));
    runtime.register_provider("broken", Arc::clone(&broken) as Arc<dyn DecisionProvider>);

    let error = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("broken"),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, TdmError::Server { status: 500, .. }));
    assert_eq!(broken.calls(), 2, "server_max total attempts");
}

#[tokio::test]
async fn client_errors_are_never_retried() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |config| {
        config.retry.network_max = 3;
        config.retry.server_max = 3;
        config.retry.backoff_ms = 1;
    });
    let broken = Arc::new(StaticErrorProvider::new(TdmError::Client {
        status: 400,
        code: Some("bad_request".to_owned()),
        message: "no".to_owned(),
    }));
    runtime.register_provider("broken", Arc::clone(&broken) as Arc<dyn DecisionProvider>);

    let error = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("broken"),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, TdmError::Client { status: 400, .. }));
    assert_eq!(broken.calls(), 1, "exactly one attempt");
}

// ---------------------------------------------------------------------------
// Circuit breaker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn circuit_opens_and_fails_fast() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |config| {
        config.retry.network_max = 1; // one attempt per call
        config.retry.backoff_ms = 1;
        config.retry.circuit_threshold = 2;
        // production cooldown default (60 s) untouched: the circuit stays
        // open for the rest of this test.
    });
    let broken = Arc::new(StaticErrorProvider::new(network_error()));
    runtime.register_provider("broken", Arc::clone(&broken) as Arc<dyn DecisionProvider>);

    let first = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("broken"),
        )
        .await
        .unwrap_err();
    assert_eq!(first, network_error());
    assert_eq!(broken.calls(), 1);

    let second = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("broken"),
        )
        .await
        .unwrap_err();
    assert_eq!(second, network_error());
    assert_eq!(broken.calls(), 2);

    let third = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("broken"),
        )
        .await
        .unwrap_err();
    assert_eq!(
        third,
        TdmError::Server {
            status: 503,
            message: "circuit open".to_owned()
        }
    );
    assert_eq!(broken.calls(), 2, "open circuit fails fast, no invocation");

    let rows = runtime.audit_recent(AuditFilter::default(), 10).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("circuit open")
    );
}

#[tokio::test]
async fn circuit_half_open_probe_recovers() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |config| {
        config.retry.network_max = 1;
        config.retry.backoff_ms = 1;
        config.retry.circuit_threshold = 2;
        config.retry.circuit_cooldown_ms = 100;
    });
    let flaky = Arc::new(FlakyProvider::new(2, network_error()));
    runtime.register_provider("flaky", Arc::clone(&flaky) as Arc<dyn DecisionProvider>);

    for _ in 0..2 {
        runtime
            .judge(
                SessionCtx::new("t", "s1"),
                noul_request(serde_json::json!(1)),
                Some("flaky"),
            )
            .await
            .unwrap_err();
    }
    assert_eq!(flaky.calls(), 2);

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // Cooldown elapsed: a single half-open probe is admitted and succeeds.
    let probe = runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(1)),
            Some("flaky"),
        )
        .await
        .expect("probe succeeds");
    assert!(!probe.from_cache);
    assert_eq!(flaky.calls(), 3);

    // Success closed the circuit: traffic flows again. (A different state
    // keeps this call off the cache the probe just filled.)
    runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!(2)),
            Some("flaky"),
        )
        .await
        .expect("circuit closed after probe");
    assert_eq!(flaky.calls(), 4);
}

#[tokio::test]
async fn client_errors_never_trip_the_circuit() {
    let dir = TempDir::new().unwrap();
    let mut runtime = setup(&dir, |config| {
        config.retry.circuit_threshold = 1;
        config.retry.backoff_ms = 1;
    });
    let broken = Arc::new(StaticErrorProvider::new(TdmError::Client {
        status: 422,
        code: Some("validation".to_owned()),
        message: "no".to_owned(),
    }));
    runtime.register_provider("broken", Arc::clone(&broken) as Arc<dyn DecisionProvider>);

    for _ in 0..3 {
        let error = runtime
            .judge(
                SessionCtx::new("t", "s1"),
                noul_request(serde_json::json!(1)),
                Some("broken"),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, TdmError::Client { status: 422, .. }),
            "got {error:?}"
        );
    }
    assert_eq!(
        broken.calls(),
        3,
        "never fails fast: Client is not a circuit failure"
    );
}

// ---------------------------------------------------------------------------
// Audit + stats
// ---------------------------------------------------------------------------

#[tokio::test]
async fn audit_rows_capture_success_cache_and_error_paths() {
    let dir = TempDir::new().unwrap();
    let runtime = setup(&dir, |_config| {});

    let request = noul_request(serde_json::json!({ "repo": "tdm" }));
    let first = runtime
        .judge(SessionCtx::new("t", "s1"), request.clone(), None)
        .await
        .unwrap();
    runtime
        .judge(SessionCtx::new("t", "s1"), request.clone(), None)
        .await
        .unwrap();
    runtime
        .judge(
            SessionCtx::new("t", "s2"),
            noul_request(serde_json::json!({})),
            Some("nope"),
        )
        .await
        .unwrap_err();

    let rows = runtime.audit_recent(AuditFilter::default(), 100).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.windows(2).all(|w| w[0].id > w[1].id),
        "newest first: {:?}",
        rows.iter().map(|r| r.id).collect::<Vec<_>>()
    );

    // Newest row: the unknown-provider failure.
    assert_eq!(rows[0].provider, "nope");
    assert_eq!(rows[0].session_id, "s2");
    assert!(rows[0].error.is_some());
    assert!(rows[0].answers_json.is_none());
    assert_eq!(rows[0].input_tokens, 0);

    // Cache-hit row.
    assert!(rows[1].cached);
    assert_eq!(rows[1].provider, "mock");
    assert_eq!(rows[1].model.as_deref(), Some("mock-1"));
    assert_eq!(
        rows[1].request_hash,
        cache_key("mock", Some("mock-1"), &request)
    );
    assert!(rows[1].error.is_none());

    // Success row.
    assert!(!rows[2].cached);
    assert_eq!(rows[2].harness, "t");
    assert_eq!(rows[2].session_id, "s1");
    assert_eq!(rows[2].input_tokens, first.result.usage.input_tokens);
    assert_eq!(rows[2].output_tokens, first.result.usage.output_tokens);
    assert!(rows[2].error.is_none());

    // Filters.
    let by_session = runtime
        .audit_recent(
            AuditFilter {
                session_id: Some("s2".to_owned()),
                ..AuditFilter::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(by_session.len(), 1);
    assert_eq!(by_session[0].provider, "nope");

    let by_provider = runtime
        .audit_recent(
            AuditFilter {
                provider: Some("mock".to_owned()),
                ..AuditFilter::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(by_provider.len(), 2);

    let by_harness = runtime
        .audit_recent(
            AuditFilter {
                harness: Some("other".to_owned()),
                ..AuditFilter::default()
            },
            100,
        )
        .unwrap();
    assert!(by_harness.is_empty());

    let limited = runtime.audit_recent(AuditFilter::default(), 1).unwrap();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].id, rows[0].id);
}

#[tokio::test]
async fn stats_aggregate_per_provider_and_harness() {
    let dir = TempDir::new().unwrap();
    let runtime = setup(&dir, |_config| {});

    let request = noul_request(serde_json::json!({ "repo": "tdm" }));
    let first = runtime
        .judge(SessionCtx::new("t", "s1"), request.clone(), None)
        .await
        .unwrap();
    runtime
        .judge(SessionCtx::new("t", "s1"), request, None)
        .await
        .unwrap();
    runtime
        .judge(
            SessionCtx::new("t", "s1"),
            noul_request(serde_json::json!({})),
            Some("nope"),
        )
        .await
        .unwrap_err();
    // A second harness splits the aggregate.
    runtime
        .judge(
            SessionCtx::new("cli", "s9"),
            noul_request(serde_json::json!({})),
            None,
        )
        .await
        .unwrap();

    let stats = runtime.stats().unwrap();
    assert_eq!(stats.len(), 3, "mock/t, mock/cli, nope/t");

    let mock_t = stats
        .iter()
        .find(|s| s.provider == "mock" && s.harness == "t")
        .unwrap();
    assert_eq!(mock_t.calls, 2);
    assert_eq!(mock_t.cache_hits, 1);
    assert_eq!(mock_t.input_tokens, first.result.usage.input_tokens * 2);
    assert_eq!(mock_t.output_tokens, first.result.usage.output_tokens * 2);
    assert_eq!(
        mock_t.billable_input_tokens, first.result.usage.input_tokens,
        "the cache-hit replay is excluded from billable"
    );
    assert_eq!(
        mock_t.billable_output_tokens,
        first.result.usage.output_tokens
    );
    assert!(mock_t.avg_latency_ms >= 0.0);

    let nope_t = stats
        .iter()
        .find(|s| s.provider == "nope" && s.harness == "t")
        .unwrap();
    assert_eq!(nope_t.calls, 1);
    assert_eq!(nope_t.cache_hits, 0);
    assert_eq!(nope_t.input_tokens, 0);

    let mock_cli = stats
        .iter()
        .find(|s| s.provider == "mock" && s.harness == "cli")
        .unwrap();
    assert_eq!(mock_cli.calls, 1);
    assert_eq!(mock_cli.cache_hits, 0);
}
