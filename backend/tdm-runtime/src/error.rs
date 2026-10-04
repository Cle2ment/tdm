//! Runtime-level failures: config resolution, registry construction, and
//! storage. Provider-side judgment failures use [`tdm_core::TdmError`].

use tdm_core::TdmError;

/// Errors raised while loading config or building/querying a
/// [`crate::Runtime`].
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// `$TDM_CONFIG_DIR` is unset and the platform has no config directory.
    #[error("cannot determine the tdm config directory; set TDM_CONFIG_DIR")]
    ConfigDir,
    /// Config or auth file I/O failure (other than a missing optional file).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// `config.toml` / `auth.toml` is not valid TOML or violates the schema.
    #[error("config error: {0}")]
    Toml(#[from] toml::de::Error),
    /// Audit/cache database failure.
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    /// JSON (de)serialization failure (e.g. a corrupt cached result).
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// A provider-side [`TdmError`] surfaced through a runtime entry point.
    #[error(transparent)]
    Judge(#[from] TdmError),
    /// Registry / routing misconfiguration with a human-readable message.
    #[error("{0}")]
    Other(String),
}

impl RuntimeError {
    pub(crate) fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }
}
