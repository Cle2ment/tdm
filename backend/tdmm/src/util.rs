//! Shared helpers for the M1 command surface: path resolution, secret
//! redaction, table rendering, and the audit-query runtime.

use std::path::{Path, PathBuf};

use tdm_runtime::{ProviderConfig, Runtime, TdmConfig};

/// Registry name of the synthetic provider that satisfies
/// [`Runtime::from_config`]'s default-route check for query-only commands.
const QUERY_PROVIDER: &str = "__tdmm_query__";

/// Resolves the config directory exactly like the runtime does
/// (`$TDM_CONFIG_DIR` when set to a non-empty value, else the platform
/// config dir + `tdm`).
///
/// # Errors
/// When no override is set and the platform has no config directory.
pub fn config_dir() -> anyhow::Result<PathBuf> {
    Ok(tdm_runtime::config::config_dir()?)
}

/// Resolved `(config.toml, auth.toml)` paths.
///
/// # Errors
/// See [`config_dir`].
pub fn config_paths() -> anyhow::Result<(PathBuf, PathBuf)> {
    let dir = config_dir()?;
    Ok((dir.join("config.toml"), dir.join("auth.toml")))
}

/// Redacts a secret for display: first 4 characters + ellipsis. The full
/// value is never rendered.
#[must_use]
pub fn redact(secret: &str) -> String {
    let prefix: String = secret.chars().take(4).collect();
    format!("{prefix}…")
}

/// Truncates `text` to `max` characters for table display, appending an
/// ellipsis when cut.
#[must_use]
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

/// Expands a leading `~` (or `~\`) to the home directory; other paths pass
/// through unchanged. Mirrors the runtime's internal helper, which is not
/// part of its public API.
#[must_use]
pub fn expand_tilde(path: &Path) -> PathBuf {
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

/// `Some(value)` when the env var is set to a non-empty string.
#[must_use]
pub fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Opens a runtime for audit queries (`logs` / `stats`).
///
/// Queries never route a judgment, so the default-route validation of
/// [`Runtime::from_config`] is satisfied with a synthetic mock entry — a
/// keyless or misconfigured default provider must not block reads. The
/// audit database path still comes from the real config.
///
/// # Errors
/// Config resolution / parse failures, or an unopenable audit database.
pub fn open_query_runtime() -> anyhow::Result<Runtime> {
    let mut config = TdmConfig::load()?;
    config.provider.insert(
        QUERY_PROVIDER.to_owned(),
        ProviderConfig {
            provider_type: "mock".to_owned(),
            ..ProviderConfig::default()
        },
    );
    config.defaults.provider = QUERY_PROVIDER.to_owned();
    Ok(Runtime::from_config(config)?)
}

/// Prints `headers` and `rows` as a two-space-padded table. Each row must
/// have exactly `headers.len()` cells.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let widths: Vec<usize> = (0..headers.len())
        .map(|column| {
            headers
                .get(column)
                .map_or(0, |header| header.chars().count())
                .max(
                    rows.iter()
                        .filter_map(|row| row.get(column))
                        .map(|cell| cell.chars().count())
                        .max()
                        .unwrap_or(0),
                )
        })
        .collect();
    let header_cells: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, header)| {
            let width = widths[i];
            format!("{header:<width$}")
        })
        .collect();
    println!("{}", header_cells.join("  "));
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let width = widths[i];
                format!("{cell:<width$}")
            })
            .collect();
        println!("{}", cells.join("  "));
    }
}
