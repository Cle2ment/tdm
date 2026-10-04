//! `tdmm call` — single-shot judgment (plan §7).
//!
//! Request JSON (file or stdin) -> [`DecisionRequest`] -> provider
//! `judge` -> DecisionResult JSON on stdout. Errors go to stderr; exit codes
//! follow the caller-side vs. everything-else split documented in [`crate`].

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use tdm_core::{DecisionProvider, DecisionRequest, TdmError};

use crate::cli::ProviderArg;
use crate::{EXIT_CLIENT, EXIT_FAILURE, EXIT_OK};

/// API key environment variables consulted for default provider selection
/// and by [`tdm_provider_jev::JevConfig::from_env`]. Never printed.
const JEV_KEY_ENV: &str = "TDM_JEV_API_KEY";
const TYPESAFE_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// Call failure classified onto the exit-code contract.
enum CallError {
    /// Caller-side: malformed request JSON, failed validation, or a provider
    /// [`TdmError::Client`]. Exits [`EXIT_CLIENT`].
    Client(String),
    /// Everything else. Exits [`EXIT_FAILURE`].
    Other(anyhow::Error),
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

    let provider = build_provider(provider)?;
    let result = provider.judge(request).await.map_err(CallError::from)?;

    let output = if compact {
        serde_json::to_string(&result)
    } else {
        serde_json::to_string_pretty(&result)
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

/// Builds the selected provider; `None` applies the env-based default.
fn build_provider(choice: Option<ProviderArg>) -> Result<Arc<dyn DecisionProvider>, CallError> {
    match choice.unwrap_or_else(default_provider) {
        ProviderArg::Mock => Ok(Arc::new(tdm_provider_mock::MockProvider::new())),
        ProviderArg::Jev => {
            let config = tdm_provider_jev::JevConfig::from_env()
                .map_err(|error| CallError::Other(error.into()))?;
            Ok(Arc::new(tdm_provider_jev::JevProvider::new(config)))
        }
    }
}

/// Default provider: `jev` when an API key env var is set, else `mock`.
fn default_provider() -> ProviderArg {
    let key_set = std::env::var(JEV_KEY_ENV).is_ok() || std::env::var(TYPESAFE_KEY_ENV).is_ok();
    if key_set {
        ProviderArg::Jev
    } else {
        ProviderArg::Mock
    }
}
