//! Config directory resolution and skeleton file content (plan §6, ADR-0002).

use std::path::PathBuf;

use anyhow::Context;

/// Environment override for the config directory (testability + ops use).
pub const CONFIG_DIR_ENV: &str = "TDM_CONFIG_DIR";

/// Resolves the tdm config directory: `$TDM_CONFIG_DIR` when set to a
/// non-empty value, else the platform config dir + `tdm`
/// (Windows: `%APPDATA%\tdm`; Unix: follows `dirs` / XDG conventions).
///
/// # Errors
/// When no override is set and the platform has no config directory.
pub fn config_dir() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var(CONFIG_DIR_ENV) {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    dirs::config_dir()
        .map(|dir| dir.join("tdm"))
        .context("cannot determine the tdm config directory; set TDM_CONFIG_DIR")
}

/// Skeleton `config.toml` — layout mirrors plan §6. Parsing itself is M1.
pub const CONFIG_TOML: &str = r#"# TDM configuration — created by `tdmm init`.
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

/// Skeleton `auth.toml` — secrets only, never commit. `tdmm keys` lands in M1.
pub const AUTH_TOML: &str = r#"# TDM auth secrets — NEVER commit or share this file.
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
