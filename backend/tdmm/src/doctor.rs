//! `tdmm doctor` — configuration / key / provider-health / audit-db
//! diagnostics (plan §7).
//!
//! Checks, per configured provider: construction + `health()` (ok,
//! latency, version), key presence (redacted). Plus config.toml / auth.toml
//! parse status and audit-database writability. Exits 1 when the default
//! provider is unhealthy (unknown, disabled, keyless, or a failed probe).

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use tdm_core::{DecisionProvider, HealthReport};
use tdm_provider_jev::{JevConfig, JevProvider};
use tdm_provider_mock::MockProvider;
use tdm_runtime::{ProviderConfig, TdmConfig, api_key_env};
use toml_edit::DocumentMut;

use crate::util::{config_paths, expand_tilde, non_empty_env, redact};
use crate::{EXIT_FAILURE, EXIT_OK};

/// Parse status of one file on disk.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileCheck {
    path: String,
    status: String,
}

/// Diagnostics for one `[provider.*]` entry.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderCheck {
    name: String,
    /// Provider implementation (`type` in config).
    kind: String,
    enabled: bool,
    /// Redacted key, when one resolves.
    key: Option<String>,
    /// Where the key came from (env var name / auth.toml).
    key_source: Option<String>,
    /// Human-readable status when no health probe ran.
    status: String,
    /// `None` when the provider is not constructible (skipped by the
    /// runtime); otherwise the probe verdict.
    healthy: Option<bool>,
    health: Option<HealthReport>,
}

/// Audit-database writability probe.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditDbCheck {
    path: String,
    writable: bool,
    error: Option<String>,
}

/// Full doctor report (JSON shape and human rendering share this).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DoctorReport {
    config_dir: String,
    config: FileCheck,
    auth: FileCheck,
    audit_db: AuditDbCheck,
    providers: Vec<ProviderCheck>,
    default_provider: String,
    default_healthy: bool,
}

/// Runs `tdmm doctor`; returns the process exit code.
pub async fn run(json: bool) -> u8 {
    match check().await {
        Ok(report) => {
            if json {
                match serde_json::to_string_pretty(&report) {
                    Ok(text) => println!("{text}"),
                    Err(error) => {
                        eprintln!("error: {error}");
                        return EXIT_FAILURE;
                    }
                }
            } else {
                print_human(&report);
            }
            if report.default_healthy {
                EXIT_OK
            } else {
                EXIT_FAILURE
            }
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            EXIT_FAILURE
        }
    }
}

/// Collects the full report.
///
/// # Errors
/// Only when the config directory itself cannot be resolved or a file is
/// unreadable (as opposed to unparseable — parse failures are reported as
/// per-file status and doctor continues on built-in defaults).
async fn check() -> anyhow::Result<DoctorReport> {
    let (config_path, auth_path) = config_paths()?;
    let config_dir = config_path
        .parent()
        .unwrap_or(Path::new(""))
        .display()
        .to_string();

    let config_status = match std::fs::read_to_string(&config_path) {
        Ok(text) => match TdmConfig::from_toml_str(&text) {
            Ok(_) => "ok".to_owned(),
            Err(error) => format!("error: {error}"),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "missing (built-in defaults apply)".to_owned()
        }
        Err(error) => return Err(error.into()),
    };

    let auth_status = match std::fs::read_to_string(&auth_path) {
        Ok(text) => match text.parse::<DocumentMut>() {
            Ok(_) => "ok".to_owned(),
            Err(error) => format!("error: {error}"),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing".to_owned(),
        Err(error) => return Err(error.into()),
    };

    let merged = match TdmConfig::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!(
                "warning: cannot load the merged tdm config ({error}); \
                 checking built-in defaults"
            );
            TdmConfig::default()
        }
    };

    let mut providers = Vec::new();
    for (name, entry) in &merged.provider {
        providers.push(check_provider(&merged, name, entry).await);
    }

    let audit_db_path = expand_tilde(&merged.audit.path);
    let (writable, error) = check_audit_writable(&audit_db_path);

    let default_provider = merged.defaults.provider.clone();
    let default_healthy = providers
        .iter()
        .find(|provider| provider.name == default_provider)
        .is_some_and(|provider| provider.healthy == Some(true));

    Ok(DoctorReport {
        config_dir,
        config: FileCheck {
            path: config_path.display().to_string(),
            status: config_status,
        },
        auth: FileCheck {
            path: auth_path.display().to_string(),
            status: auth_status,
        },
        audit_db: AuditDbCheck {
            path: audit_db_path.display().to_string(),
            writable,
            error,
        },
        providers,
        default_provider,
        default_healthy,
    })
}

/// Checks one provider entry: key presence (redacted), then construction
/// and a `health()` probe for known types. Keyless jev entries are excluded
/// with a warning, mirroring the runtime registry.
async fn check_provider(config: &TdmConfig, name: &str, entry: &ProviderConfig) -> ProviderCheck {
    let enabled = entry.enabled != Some(false);
    let (key, key_source) = resolve_key(config, name);
    let mut check = ProviderCheck {
        name: name.to_owned(),
        kind: entry.provider_type.clone(),
        enabled,
        key: key.as_deref().map(redact),
        key_source,
        status: String::new(),
        healthy: None,
        health: None,
    };

    if !enabled {
        check.status = "disabled (enabled = false); excluded from the registry".to_owned();
        return check;
    }

    match entry.provider_type.as_str() {
        "mock" => {
            let health = MockProvider::new().health().await;
            check.healthy = Some(health.ok);
            check.health = Some(health);
        }
        "jev" => match key {
            None => {
                let env_var = api_key_env(name);
                check.status = format!(
                    "no API key (set {env_var} or auth.toml [{name}].api_key); \
                     excluded from the registry"
                );
                eprintln!(
                    "warning: provider {name:?} has no API key ({env_var}); \
                     excluded from the registry"
                );
            }
            Some(key) => {
                // Same override chain as the runtime registry builder;
                // absent keys fall back to the provider's pinned defaults.
                let jev_config = JevConfig {
                    api_key: key,
                    endpoint: entry
                        .endpoint
                        .clone()
                        .unwrap_or_else(|| JevConfig::default().endpoint),
                    model: entry
                        .model
                        .clone()
                        .unwrap_or_else(|| JevConfig::default().model),
                    timeout: Duration::from_millis(entry.timeout_ms.unwrap_or(10_000).max(1)),
                };
                let health = JevProvider::new(jev_config).health().await;
                check.healthy = Some(health.ok);
                check.health = Some(health);
            }
        },
        other => {
            check.status = format!("unknown provider type {other:?}; skipped by the runtime");
        }
    }
    check
}

/// Auth resolution with provenance: env → auth.toml → legacy
/// `TYPESAFE_API_KEY` (jev only). Mirrors the runtime's `pub(crate)` helper.
fn resolve_key(config: &TdmConfig, name: &str) -> (Option<String>, Option<String>) {
    let env_var = api_key_env(name);
    if let Some(key) = non_empty_env(&env_var) {
        return (Some(key), Some(format!("env {env_var}")));
    }
    if let Some(key) = config.auth.get(name).filter(|key| !key.is_empty()) {
        return (Some(key.clone()), Some("auth.toml".to_owned()));
    }
    let is_jev = name == "jev"
        || config
            .provider
            .get(name)
            .is_some_and(|entry| entry.provider_type == "jev");
    if is_jev {
        if let Some(key) = non_empty_env(tdm_runtime::config::LEGACY_TYPESAFE_KEY_ENV) {
            return (Some(key), Some("env TYPESAFE_API_KEY (legacy)".to_owned()));
        }
    }
    (None, None)
}

/// Probes that the audit database location is writable: creates the parent
/// directory (the runtime does the same on first write) and opens the file
/// for append.
fn check_audit_writable(path: &Path) -> (bool, Option<String>) {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                return (
                    false,
                    Some(format!("cannot create {}: {error}", parent.display())),
                );
            }
        }
    }
    match std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
    {
        Ok(_) => (true, None),
        Err(error) => (false, Some(error.to_string())),
    }
}

/// Human-readable rendering; warnings have already gone to stderr during
/// the check.
fn print_human(report: &DoctorReport) {
    println!("config dir: {}", report.config_dir);
    println!("config: {} ({})", report.config.status, report.config.path);
    println!("auth: {} ({})", report.auth.status, report.auth.path);
    let audit_state = if report.audit_db.writable {
        "writable".to_owned()
    } else {
        format!(
            "NOT writable ({})",
            report.audit_db.error.as_deref().unwrap_or("unknown error")
        )
    };
    println!("audit db: {audit_state} ({})", report.audit_db.path);

    println!("providers:");
    for provider in &report.providers {
        let mut line = format!("  {}: {}", provider.name, provider.kind);
        if !provider.enabled {
            line.push_str(" [disabled]");
        }
        if let Some(key) = &provider.key {
            match &provider.key_source {
                Some(source) => line.push_str(&format!(" — key {key} ({source})")),
                None => line.push_str(&format!(" — key {key}")),
            }
        }
        if let Some(health) = &provider.health {
            let version = health
                .version
                .as_deref()
                .map_or_else(String::new, |version| format!(", version {version}"));
            let state = if health.ok { "ok" } else { "FAILED" };
            line.push_str(&format!(
                " — health {state} ({} ms{version})",
                health.latency_ms
            ));
        } else {
            line.push_str(&format!(" — {}", provider.status));
        }
        println!("{line}");
    }

    if report.default_healthy {
        println!("default provider: {} — healthy", report.default_provider);
    } else {
        let reason = report
            .providers
            .iter()
            .find(|provider| provider.name == report.default_provider)
            .map_or_else(
                || "not defined under [provider.*]".to_owned(),
                |provider| {
                    if provider.status.is_empty() {
                        "health check failed".to_owned()
                    } else {
                        provider.status.clone()
                    }
                },
            );
        println!(
            "default provider: {} — UNHEALTHY ({reason})",
            report.default_provider
        );
    }
}
