//! `tdm-napi` — native Node.js binding for the TDM decision runtime, published
//! as the npm package `@typedecision/runtime`.
//!
//! Exports (see `index.d.ts` for the shipped TypeScript surface):
//!
//! - [`judge`]: parse a [`tdm_core::DecisionRequest`] from JSON, route it to
//!   the selected [`tdm_core::DecisionProvider`], and return the
//!   [`tdm_core::DecisionResult`] as JSON.
//! - [`health`]: probe the selected provider and return its
//!   [`tdm_core::HealthReport`] as JSON.
//!
//! # Provider selection
//!
//! Per call: an explicit `opts.provider` (`"jev"` | `"mock"`) wins; otherwise
//! jev is used when `TDM_JEV_API_KEY` or `TYPESAFE_API_KEY` is set; otherwise
//! the deterministic mock. See [`select_provider_kind`].
//!
//! # Threading
//!
//! All provider work runs on one shared multi-thread tokio runtime
//! ([`RUNTIME`], lazily initialized with `OnceLock`) — never a per-call
//! runtime.
//!
//! # Error mapping
//!
//! - a request/options JSON payload that fails to parse, an unknown provider
//!   name, or jev without a configured key -> `napi::Error` with
//!   [`napi::Status::InvalidArg`]
//! - [`tdm_core::TdmError`] -> `napi::Error` with
//!   [`napi::Status::GenericFailure`], reason prefixed with the error class
//!   (`[client]` / `[server]` / `[network]` / `[quality]` / `[unsupported]`)

use std::sync::{Arc, OnceLock};

use napi::{Error, Result, Status};
use napi_derive::napi;
use serde::Deserialize;
use serde_json::Value;
use tdm_core::{DecisionProvider, DecisionRequest, TdmError};
use tdm_provider_jev::{JevConfig, JevProvider};
use tdm_provider_mock::MockProvider;

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

/// Which backing provider a call should route to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderKind {
    Jev,
    Mock,
}

/// M0 env probe: jev is considered available when either conventional key
/// variable is set. (Key resolution moves into the tdm config system in M1.)
fn jev_key_available() -> bool {
    std::env::var_os("TDM_JEV_API_KEY").is_some() || std::env::var_os("TYPESAFE_API_KEY").is_some()
}

/// Resolves the provider kind for one call: an explicit `provider` option
/// wins; otherwise jev when an API key is present in the environment;
/// otherwise the deterministic mock.
fn select_provider_kind(explicit: Option<&str>, jev_available: bool) -> Result<ProviderKind> {
    match explicit {
        Some("jev") if jev_available => Ok(ProviderKind::Jev),
        Some("jev") => Err(Error::new(
            Status::InvalidArg,
            "provider \"jev\" requested but no API key is set (TDM_JEV_API_KEY or \
             TYPESAFE_API_KEY)",
        )),
        Some("mock") => Ok(ProviderKind::Mock),
        Some(other) => Err(Error::new(
            Status::InvalidArg,
            format!("unknown provider {other:?}: expected \"jev\" or \"mock\""),
        )),
        None if jev_available => Ok(ProviderKind::Jev),
        None => Ok(ProviderKind::Mock),
    }
}

/// Instantiates the provider for one call. Providers are cheap, stateless
/// values here; session/audit wiring (M1) will own longer-lived instances.
fn build_provider(kind: ProviderKind) -> Result<Arc<dyn DecisionProvider>> {
    match kind {
        ProviderKind::Mock => Ok(Arc::new(MockProvider::new())),
        ProviderKind::Jev => {
            let config = JevConfig::from_env().map_err(|error| {
                Error::new(
                    Status::InvalidArg,
                    format!("provider \"jev\" is not configured: {error}"),
                )
            })?;
            Ok(Arc::new(JevProvider::new(config)))
        }
    }
}

/// Per-call options accepted from JS.
///
/// `session` is accepted for compatibility with the adapter-facing
/// `JudgeOptions` surface (`adapters/tdm-client`) but is IGNORED for now:
/// harness/session tagging is runtime wiring scheduled for M1 (audit rows,
/// cache entries, and cost stats keyed per harness session).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JudgeOptions {
    /// Explicit provider override: `"jev"` or `"mock"`.
    #[serde(default)]
    provider: Option<String>,
    /// Harness + session identity. Accepted but IGNORED (wired up in M1).
    #[allow(dead_code)] // deliberately unused: session tagging lands with M1
    #[serde(default)]
    session: Option<Value>,
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

/// Judges one batch of questions against the given state.
///
/// The request and options are plain JSON objects (wire shape per
/// `@typedecision/contract`); the resolved [`tdm_core::DecisionResult`] is
/// returned as JSON. Rejects with a class-prefixed error message on provider
/// failure.
///
/// # Errors
/// [`Status::InvalidArg`] for malformed input or an unusable provider
/// selection; [`Status::GenericFailure`] for provider/serialization failures.
#[napi]
pub async fn judge(req: Value, opts: Option<Value>) -> Result<Value> {
    let request = parse_request(req)?;
    let options = parse_options(opts)?;
    let kind = select_provider_kind(options.provider.as_deref(), jev_key_available())?;
    let provider = build_provider(kind)?;

    let result = runtime()
        .spawn(async move { provider.judge(request).await })
        .await
        .map_err(|error| {
            Error::new(
                Status::GenericFailure,
                format!("judge task failed to complete: {error}"),
            )
        })?
        .map_err(tdm_error_to_napi)?;

    serde_json::to_value(&result).map_err(|error| {
        Error::new(
            Status::GenericFailure,
            format!("failed to serialize DecisionResult: {error}"),
        )
    })
}

/// Probes the selected provider's liveness/connectivity.
///
/// Provider selection follows the same rules as [`judge`] (no explicit
/// override is accepted on this export). Returns the
/// [`tdm_core::HealthReport`] as JSON.
///
/// # Errors
/// [`Status::InvalidArg`] when jev is selected but not configured.
#[napi]
pub async fn health() -> Result<Value> {
    let kind = select_provider_kind(None, jev_key_available())?;
    let provider = build_provider(kind)?;

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
    use super::*;

    #[test]
    fn explicit_mock_wins_over_environment() {
        assert_eq!(
            select_provider_kind(Some("mock"), true).unwrap(),
            ProviderKind::Mock
        );
        assert_eq!(
            select_provider_kind(Some("mock"), false).unwrap(),
            ProviderKind::Mock
        );
    }

    #[test]
    fn explicit_jev_requires_a_key() {
        assert_eq!(
            select_provider_kind(Some("jev"), true).unwrap(),
            ProviderKind::Jev
        );
        let error = select_provider_kind(Some("jev"), false).unwrap_err();
        assert_eq!(error.status, Status::InvalidArg);
        assert!(error.reason.contains("TDM_JEV_API_KEY"));
    }

    #[test]
    fn no_explicit_provider_falls_back_to_jev_then_mock() {
        assert_eq!(select_provider_kind(None, true).unwrap(), ProviderKind::Jev);
        assert_eq!(
            select_provider_kind(None, false).unwrap(),
            ProviderKind::Mock
        );
    }

    #[test]
    fn unknown_provider_name_is_rejected() {
        let error = select_provider_kind(Some("claude"), true).unwrap_err();
        assert_eq!(error.status, Status::InvalidArg);
        assert!(error.reason.contains("claude"));
    }

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
        let error = parse_options(Some(serde_json::json!({ "provider": 3 }))).unwrap_err();
        assert_eq!(error.status, Status::InvalidArg);
    }
}
