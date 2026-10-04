//! `tdmm call` — single-shot judgment through the tdm-runtime engine
//! (plan §7).
//!
//! Request JSON (file or stdin) -> [`DecisionRequest`] -> [`Runtime::judge`]
//! (registry routing, exact-hash cache, retry/circuit, audit) ->
//! DecisionResult JSON on stdout. Cache hits are transparent: the printed
//! result is byte-identical for hits and misses; only `tdmm logs` shows the
//! `cached` flag. Errors go to stderr; exit codes follow the caller-side
//! vs. everything-else split documented in [`crate`].

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Context;
use tdm_core::{DecisionRequest, TdmError};
use tdm_runtime::{ProviderConfig, Runtime, SessionCtx, TdmConfig, api_key_env};

use crate::cli::ProviderArg;
use crate::util::non_empty_env;
use crate::{EXIT_CLIENT, EXIT_FAILURE, EXIT_OK};

/// API key environment variables consulted by the bare-install default
/// heuristic. Never printed.
const JEV_KEY_ENV: &str = "TDM_JEV_API_KEY";
const TYPESAFE_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// Harness label attached to every audit row produced by the CLI.
const HARNESS: &str = "tdmm";

/// Call failure classified onto the exit-code contract.
#[derive(Debug)]
enum CallError {
    /// Caller-side: malformed request JSON, failed validation, or a provider
    /// [`TdmError::Client`]. Exits [`EXIT_CLIENT`].
    Client(String),
    /// Everything else. Exits [`EXIT_FAILURE`].
    Other(anyhow::Error),
}

impl CallError {
    /// Wraps any std error (config / runtime failures) as [`Self::Other`].
    fn other<E: Into<anyhow::Error>>(error: E) -> Self {
        Self::Other(error.into())
    }
}

impl From<TdmError> for CallError {
    fn from(error: TdmError) -> Self {
        match error {
            TdmError::Client { .. } => Self::Client(error.to_string()),
            other => Self::Other(other.into()),
        }
    }
}

impl From<anyhow::Error> for CallError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

/// Runs `tdmm call`; returns the process exit code.
pub async fn run(file: Option<PathBuf>, provider: Option<ProviderArg>, compact: bool) -> u8 {
    match call(file, provider, compact).await {
        Ok(()) => EXIT_OK,
        Err(CallError::Client(message)) => {
            eprintln!("error: {message}");
            EXIT_CLIENT
        }
        Err(CallError::Other(error)) => {
            eprintln!("error: {error:#}");
            EXIT_FAILURE
        }
    }
}

/// The full call pipeline; `Ok` means the result JSON was printed.
async fn call(
    file: Option<PathBuf>,
    provider: Option<ProviderArg>,
    compact: bool,
) -> Result<(), CallError> {
    let request = read_request(file.as_deref())?;

    // Local shape validation before any provider construction or traffic:
    // identical behavior regardless of the selected provider.
    for question in &request.questions {
        if let Err(validation) = question.validate() {
            let id = &question.id;
            return Err(CallError::Client(format!("question {id}: {validation}")));
        }
    }

    let mut config = TdmConfig::load().map_err(CallError::other)?;
    let override_name = provider.map(ProviderArg::as_str);
    let name = match override_name {
        Some(name) => name.to_owned(),
        None => default_provider_name(&config)?,
    };
    ensure_jev_key(&config, &name)?;
    ensure_entry(&mut config, &name);
    // `Runtime::from_config` validates `[defaults].provider`; align it with
    // the provider this invocation actually routes to, so `--provider` works
    // even when the configured default itself is unavailable.
    config.defaults.provider = name;

    let runtime = Runtime::from_config(config).map_err(CallError::other)?;
    let session = SessionCtx::new(HARNESS, format!("cli-{}", std::process::id()));
    let judged = runtime.judge(session, request, override_name).await?;
    // Cache hits flow transparently: the DecisionResult JSON is identical
    // for hits and misses (the flag is visible via `tdmm logs`).
    let _ = judged.from_cache;

    let output = if compact {
        serde_json::to_string(&judged.result)
    } else {
        serde_json::to_string_pretty(&judged.result)
    }
    .context("serializing DecisionResult")?;
    println!("{output}");
    Ok(())
}

/// Reads the request payload from `path`, or from stdin for `-` / absent.
fn read_request(file: Option<&Path>) -> Result<DecisionRequest, CallError> {
    let text = match file {
        Some(path) if path != Path::new("-") => std::fs::read_to_string(path).map_err(|error| {
            CallError::Other(anyhow::anyhow!("cannot read {}: {error}", path.display()))
        })?,
        _ => {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .map_err(|error| CallError::Other(anyhow::anyhow!("cannot read stdin: {error}")))?;
            buffer
        }
    };
    serde_json::from_str(&text)
        .map_err(|error| CallError::Client(format!("invalid DecisionRequest JSON: {error}")))
}

/// Effective default provider when `--provider` is absent: `TDM_PROVIDER`
/// or an existing global config.toml wins (already layered into `config`
/// by [`TdmConfig::load`]); a bare installation keeps the M0 heuristic —
/// `jev` when an API key env var is set, else `mock`.
fn default_provider_name(config: &TdmConfig) -> Result<String, CallError> {
    let layered =
        non_empty_env(tdm_runtime::config::PROVIDER_ENV).is_some() || config_file_exists()?;
    if layered {
        return Ok(config.defaults.provider.clone());
    }
    let key_set = non_empty_env(JEV_KEY_ENV).is_some() || non_empty_env(TYPESAFE_KEY_ENV).is_some();
    Ok(if key_set { "jev" } else { "mock" }.to_owned())
}

/// Whether the global `config.toml` exists (missing files fall back to the
/// built-in defaults inside [`TdmConfig::load`], so the heuristic above can
/// only apply when no file supplies an explicit default).
fn config_file_exists() -> Result<bool, CallError> {
    let dir = tdm_runtime::config::config_dir().map_err(CallError::other)?;
    Ok(dir.join("config.toml").exists())
}

/// Pre-flight for the routed provider: a `jev` route without a resolvable
/// API key is a configuration failure (exit 1) that names the key env var.
/// Without this, the runtime would exclude the keyless entry and surface a
/// client error (exit 2) instead.
fn ensure_jev_key(config: &TdmConfig, name: &str) -> Result<(), CallError> {
    let is_jev = name == "jev"
        || config
            .provider
            .get(name)
            .is_some_and(|entry| entry.provider_type == "jev");
    if !is_jev || resolve_api_key(config, name).is_some() {
        return Ok(());
    }
    let env_var = api_key_env(name);
    Err(CallError::Other(anyhow::anyhow!(
        "provider {name:?} has no API key: set {env_var} (or auth.toml [{name}].api_key; \
         fallback: {TYPESAFE_KEY_ENV})"
    )))
}

/// Mirrors the runtime's auth resolution order (env → auth.toml → legacy
/// `TYPESAFE_API_KEY` for jev); the runtime keeps this helper `pub(crate)`.
fn resolve_api_key(config: &TdmConfig, name: &str) -> Option<String> {
    non_empty_env(&api_key_env(name))
        .or_else(|| config.auth.get(name).filter(|key| !key.is_empty()).cloned())
        .or_else(|| {
            let is_jev = name == "jev"
                || config
                    .provider
                    .get(name)
                    .is_some_and(|entry| entry.provider_type == "jev");
            if is_jev {
                non_empty_env(TYPESAFE_KEY_ENV)
            } else {
                None
            }
        })
}

/// Ensures the routed provider has a `[provider.*]` entry so the runtime
/// registry can build it: `--provider jev|mock` and the bare-install
/// heuristic refer to built-in provider types that need no explicit config.
/// Endpoint / model / timeout fall back to the provider crates' pinned
/// defaults inside the registry builder.
fn ensure_entry(config: &mut TdmConfig, name: &str) {
    if !matches!(name, "jev" | "mock") {
        return; // unknown name -> `Runtime::from_config` reports it
    }
    config
        .provider
        .entry(name.to_owned())
        .or_insert_with(|| ProviderConfig {
            provider_type: name.to_owned(),
            ..ProviderConfig::default()
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tdm_core::TdmError;

    #[test]
    fn client_errors_map_to_exit_client() {
        let error = TdmError::Client {
            status: 400,
            code: None,
            message: "nope".to_owned(),
        };
        assert!(matches!(CallError::from(error), CallError::Client(_)));

        let error = TdmError::Network {
            message: "down".to_owned(),
        };
        assert!(matches!(CallError::from(error), CallError::Other(_)));
    }

    #[test]
    fn default_provider_name_layers_and_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: single-threaded unit test; this binary has no other tests
        // reading these variables, and integration tests run as separate
        // processes with their own environment.
        unsafe {
            std::env::set_var("TDM_CONFIG_DIR", dir.path());
            std::env::remove_var("TDM_PROVIDER");
            std::env::remove_var("TDM_JEV_API_KEY");
            std::env::remove_var("TYPESAFE_API_KEY");
        }

        // Bare install, no key env -> mock.
        let config = TdmConfig::load().unwrap();
        assert_eq!(default_provider_name(&config).unwrap(), "mock");

        // Bare install, key env set -> jev (M0 heuristic).
        unsafe { std::env::set_var("TDM_JEV_API_KEY", "k") };
        let config = TdmConfig::load().unwrap();
        assert_eq!(default_provider_name(&config).unwrap(), "jev");
        unsafe { std::env::remove_var("TDM_JEV_API_KEY") };

        // TDM_PROVIDER layers over everything.
        unsafe { std::env::set_var("TDM_PROVIDER", "mock") };
        let config = TdmConfig::load().unwrap();
        assert_eq!(default_provider_name(&config).unwrap(), "mock");
        unsafe { std::env::remove_var("TDM_PROVIDER") };

        // An existing config.toml pins the default even with a key present.
        std::fs::write(
            dir.path().join("config.toml"),
            "version = 1\n[provider.mock]\ntype = \"mock\"\n[defaults]\nprovider = \"mock\"\n",
        )
        .unwrap();
        unsafe { std::env::set_var("TDM_JEV_API_KEY", "k") };
        let config = TdmConfig::load().unwrap();
        assert_eq!(default_provider_name(&config).unwrap(), "mock");
        unsafe { std::env::remove_var("TDM_JEV_API_KEY") };
    }
}
