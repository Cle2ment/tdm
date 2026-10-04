//! jev provider configuration.

use std::time::Duration;

/// Default System One endpoint (API version pinned; see docs/jev-api-notes.md).
pub const DEFAULT_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
/// Default model identifier.
pub const DEFAULT_MODEL: &str = "jev-latest";
/// Default per-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Configuration for [`crate::JevProvider`].
///
/// `Debug` is implemented manually: the `api_key` never renders — not even a
/// prefix — it always shows as `[redacted]`.
#[derive(Clone)]
pub struct JevConfig {
    /// System One endpoint URL.
    pub endpoint: String,
    /// Bearer API key.
    pub api_key: String,
    /// Model identifier sent with each request.
    pub model: String,
    /// Per-request timeout.
    pub timeout: Duration,
}

impl JevConfig {
    /// M0 convenience: defaults plus the API key from `TDM_JEV_API_KEY`,
    /// falling back to `TYPESAFE_API_KEY`.
    ///
    /// NOTE: environment-based key resolution moves into the tdm config
    /// system in M1 (`auth.toml` + layered resolution); this method is
    /// scaffolding until then.
    ///
    /// # Errors
    /// [`JevConfigError::MissingApiKey`] when neither variable is set.
    pub fn from_env() -> Result<Self, JevConfigError> {
        let api_key = std::env::var("TDM_JEV_API_KEY")
            .or_else(|_| std::env::var("TYPESAFE_API_KEY"))
            .map_err(|_| JevConfigError::MissingApiKey)?;
        Ok(Self {
            api_key,
            ..Self::default()
        })
    }
}

impl Default for JevConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_owned(),
            api_key: String::new(),
            model: DEFAULT_MODEL.to_owned(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl std::fmt::Debug for JevConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JevConfig")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"[redacted]")
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Configuration construction failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JevConfigError {
    /// Neither `TDM_JEV_API_KEY` nor `TYPESAFE_API_KEY` is set.
    #[error("missing jev API key: set TDM_JEV_API_KEY (fallback: TYPESAFE_API_KEY)")]
    MissingApiKey,
}
