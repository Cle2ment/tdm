//! TDM configuration layer (plan §6, ADR-0002).
//!
//! Resolution order implemented in M1:
//!
//! 1. built-in defaults ([`TdmConfig::default`], mirroring `tdmm init`'s
//!    skeleton),
//! 2. global `config.toml` from [`config_dir`] — a missing file is **not** an
//!    error and falls back to the defaults,
//! 3. environment overrides: `TDM_PROVIDER` (→ `[defaults].provider`) here,
//!    and `TDM_<NAME>_API_KEY` when the runtime builds its provider registry.
//!
//! Project-level `.tdm/config.toml` layering is a documented TODO for M2.
//!
//! `auth.toml` (same directory as `config.toml`) is read into
//! [`TdmConfig::auth`] at load time; keys never round-trip through
//! serialization and [`TdmConfig`]'s `Debug` output redacts them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::RuntimeError;

/// Environment override for the config directory.
pub const CONFIG_DIR_ENV: &str = "TDM_CONFIG_DIR";
/// Environment override for `[defaults].provider`.
pub const PROVIDER_ENV: &str = "TDM_PROVIDER";
/// Legacy TypeSafe key, consulted for `jev` providers only.
pub const LEGACY_TYPESAFE_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// Parsed `config.toml` plus the auth keys resolved alongside it.
///
/// Every section is optional on the wire: missing files and missing sections
/// fall back to the built-in defaults, so `tdmm init`'s skeleton and a bare
/// installation both parse.
///
/// `Debug` is implemented manually: the `auth` map holds secrets and only
/// ever renders key names, never values.
#[derive(Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct TdmConfig {
    /// Schema version of the file (`version = 1` today).
    pub version: u32,
    /// Provider entries keyed by registry name (`[provider.<name>]`).
    pub provider: BTreeMap<String, ProviderConfig>,
    /// `[defaults]` section.
    pub defaults: DefaultsConfig,
    /// `[cache]` section.
    pub cache: CacheConfig,
    /// `[retry]` section.
    pub retry: RetryConfig,
    /// `[audit]` section.
    pub audit: AuditConfig,
    /// API keys by provider name, resolved from `auth.toml` at load time.
    /// Never serialized; [`Debug`](std::fmt::Debug) only lists the key names.
    #[serde(skip)]
    pub auth: BTreeMap<String, String>,
}

impl TdmConfig {
    /// Built-in defaults (mirrors `tdmm init`'s skeleton).
    #[must_use]
    pub fn defaults() -> Self {
        Self::default()
    }

    /// Parses a `config.toml` body. Unknown keys are ignored for forward
    /// compatibility; a `version` other than `1` logs a warning and parses
    /// with the current schema.
    ///
    /// # Errors
    /// [`RuntimeError::Toml`] when the text is not valid TOML or violates
    /// the schema.
    pub fn from_toml_str(text: &str) -> Result<Self, RuntimeError> {
        let config: Self = toml::from_str(text)?;
        if config.version != 1 {
            tracing::warn!(
                version = config.version,
                "unknown config.toml version; parsing with the current schema"
            );
        }
        Ok(config)
    }

    /// Loads global config: `config.toml` + `auth.toml` from [`config_dir`],
    /// then environment overrides. Missing files fall back to defaults.
    ///
    /// # Errors
    /// [`RuntimeError::ConfigDir`] without a resolvable directory,
    /// [`RuntimeError::Io`] / [`RuntimeError::Toml`] for unreadable or
    /// invalid files.
    pub fn load() -> Result<Self, RuntimeError> {
        Self::load_from_dir(&config_dir()?)
    }

    /// Loads `config.toml` + `auth.toml` from an explicit directory, then
    /// applies environment overrides. Missing files fall back to defaults.
    ///
    /// # Errors
    /// See [`TdmConfig::load`].
    pub fn load_from_dir(dir: &Path) -> Result<Self, RuntimeError> {
        let mut config = match std::fs::read_to_string(dir.join("config.toml")) {
            Ok(text) => Self::from_toml_str(&text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error.into()),
        };

        if let Ok(text) = std::fs::read_to_string(dir.join("auth.toml")) {
            let entries: BTreeMap<String, AuthEntry> = toml::from_str(&text)?;
            for (name, entry) in entries {
                if let Some(key) = entry.api_key.filter(|key| !key.is_empty()) {
                    config.auth.insert(name, key);
                }
            }
        }

        if let Some(provider) = non_empty_env(PROVIDER_ENV) {
            config.defaults.provider = provider;
        }
        Ok(config)
    }
}

impl Default for TdmConfig {
    fn default() -> Self {
        let mut provider = BTreeMap::new();
        provider.insert("jev".to_owned(), ProviderConfig::jev_default());
        Self {
            version: 1,
            provider,
            defaults: DefaultsConfig::default(),
            cache: CacheConfig::default(),
            retry: RetryConfig::default(),
            audit: AuditConfig::default(),
            auth: BTreeMap::new(),
        }
    }
}

impl std::fmt::Debug for TdmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `auth` holds secrets: list names only, never values.
        f.debug_struct("TdmConfig")
            .field("version", &self.version)
            .field("provider", &self.provider)
            .field("defaults", &self.defaults)
            .field("cache", &self.cache)
            .field("retry", &self.retry)
            .field("audit", &self.audit)
            .field("auth_keys", &self.auth.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// One `[provider.<name>]` entry.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProviderConfig {
    /// Provider implementation: `"jev"` or `"mock"` today; unknown types are
    /// skipped with a warning when the registry is built.
    #[serde(rename = "type")]
    pub provider_type: String,
    /// Service endpoint (jev); defaults to the pinned System One URL.
    pub endpoint: Option<String>,
    /// Model identifier (jev); defaults to the provider's pinned model.
    pub model: Option<String>,
    /// Per-request timeout in milliseconds (jev).
    pub timeout_ms: Option<u64>,
    /// `false` excludes the entry from the registry entirely.
    pub enabled: Option<bool>,
}

impl ProviderConfig {
    /// Default jev entry; mirrors the `tdmm init` skeleton and the pinned
    /// defaults of `tdm-provider-jev` (keep the three in sync).
    fn jev_default() -> Self {
        Self {
            provider_type: "jev".to_owned(),
            endpoint: Some("https://api.typesafe.ai/v1/systemone".to_owned()),
            model: None,
            timeout_ms: Some(10_000),
            enabled: None,
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            provider_type: "mock".to_owned(),
            endpoint: None,
            model: None,
            timeout_ms: None,
            enabled: None,
        }
    }
}

/// `[defaults]` — routing default and adapter-side escalation threshold.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct DefaultsConfig {
    /// Default provider name used when `judge` gets no override.
    pub provider: String,
    /// Adapter-side floor: below this confidence, escalate to a human.
    pub confidence_floor: f64,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            provider: "jev".to_owned(),
            confidence_floor: 0.55,
        }
    }
}

/// `[cache]` — exact-hash response cache.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Whether the cache is consulted and filled.
    pub enabled: bool,
    /// Entry freshness window in hours; `0` disables hits entirely.
    pub ttl_hours: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl_hours: 168,
        }
    }
}

/// `[retry]` — retry and circuit-breaker knobs.
///
/// `backoff_ms` and `circuit_cooldown_ms` are runtime-internal knobs beyond
/// the file schema: the `tdmm init` skeleton omits them and the production
/// defaults (100 ms base, 60 s cooldown) apply whenever they are absent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// Total attempts allowed for `Network` errors.
    pub network_max: u32,
    /// Total attempts allowed for `Server` errors.
    pub server_max: u32,
    /// Consecutive Network/Server outcomes that open the circuit.
    pub circuit_threshold: u32,
    /// Exponential backoff base: attempt `n` waits `backoff_ms * 2^(n-1)`.
    pub backoff_ms: u64,
    /// How long an open circuit stays open before admitting a probe.
    pub circuit_cooldown_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            network_max: 3,
            server_max: 2,
            circuit_threshold: 5,
            backoff_ms: 100,
            circuit_cooldown_ms: 60_000,
        }
    }
}

/// `[audit]` — where the SQLite state database lives.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AuditConfig {
    /// Database path; `~` expands to the home directory, and the special
    /// value `:memory:` keeps everything in RAM (useful for tests).
    pub path: PathBuf,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("~/.local/share/tdm/audit.db"),
        }
    }
}

/// One `[<name>]` entry in `auth.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
struct AuthEntry {
    api_key: Option<String>,
}

/// Resolves the config directory: `$TDM_CONFIG_DIR` when set to a non-empty
/// value, else the platform config dir + `tdm` (Windows: `%APPDATA%\tdm`;
/// Unix: follows `dirs` / XDG conventions).
///
/// # Errors
/// [`RuntimeError::ConfigDir`] when no override is set and the platform has
/// no config directory.
pub fn config_dir() -> Result<PathBuf, RuntimeError> {
    if let Some(dir) = non_empty_env(CONFIG_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    dirs::config_dir()
        .map(|dir| dir.join("tdm"))
        .ok_or(RuntimeError::ConfigDir)
}

/// Environment variable holding `<name>`'s API key: `TDM_<NAME>_API_KEY`,
/// uppercased with non-alphanumerics mapped to `_`.
#[must_use]
pub fn api_key_env(provider_name: &str) -> String {
    let sanitized: String = provider_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("TDM_{sanitized}_API_KEY")
}

/// Expands a leading `~` (or `~\`) to the home directory; other paths pass
/// through unchanged. Falls back to the literal path when no home is known.
pub(crate) fn expand_home(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    let rest = if text == "~" {
        Some("")
    } else {
        text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\"))
    };
    match (rest, dirs::home_dir()) {
        (Some(""), Some(home)) => home,
        (Some(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

/// Auth resolution for one provider entry (plan §6 resolution order):
/// `TDM_<NAME>_API_KEY` env → `auth.toml` `[name].api_key` → legacy
/// `TYPESAFE_API_KEY` (jev only).
pub(crate) fn resolve_api_key(
    config: &TdmConfig,
    provider_name: &str,
    provider_type: &str,
) -> Option<String> {
    if let Some(key) = non_empty_env(&api_key_env(provider_name)) {
        return Some(key);
    }
    if let Some(key) = config.auth.get(provider_name).filter(|key| !key.is_empty()) {
        return Some(key.clone());
    }
    if provider_type == "jev" {
        if let Some(key) = non_empty_env(LEGACY_TYPESAFE_KEY_ENV) {
            return Some(key);
        }
    }
    None
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_env_sanitizes_names() {
        assert_eq!(api_key_env("jev"), "TDM_JEV_API_KEY");
        assert_eq!(api_key_env("local-mock"), "TDM_LOCAL_MOCK_API_KEY");
        assert_eq!(api_key_env("a.b"), "TDM_A_B_API_KEY");
    }

    #[test]
    fn expand_home_passthrough_without_tilde() {
        let path = Path::new("C:/data/tdm/audit.db");
        assert_eq!(expand_home(path), path.to_path_buf());
    }

    #[test]
    fn toml_missing_sections_fill_with_defaults() {
        let config = TdmConfig::from_toml_str("version = 1\n").expect("parses");
        assert_eq!(config, TdmConfig::default());
    }

    #[test]
    fn toml_unknown_keys_are_ignored() {
        let config = TdmConfig::from_toml_str(
            "version = 1\n[provider.jev]\ntype = \"jev\"\nfuture_field = true\n",
        )
        .expect("unknown keys ignored");
        assert_eq!(config.provider["jev"].provider_type, "jev");
    }
}
