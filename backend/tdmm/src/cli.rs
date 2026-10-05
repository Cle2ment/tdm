//! Command-line surface (clap derive).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// `tdmm` — TDM management CLI (full M1 surface: `init`, `call`, `use`,
/// `keys`, `doctor`, `logs`, `stats`, `cache`, `config`).
#[derive(Debug, Parser)]
#[command(
    name = "tdmm",
    version,
    about = "TDM management CLI",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create the tdm config skeleton (config.toml + auth.toml).
    ///
    /// Writes skeleton files only; the content is managed by `tdmm use` /
    /// `tdmm keys` afterwards. Idempotent: refuses to overwrite existing
    /// files unless --force is given.
    Init {
        /// Overwrite existing config.toml / auth.toml.
        #[arg(long)]
        force: bool,
    },
    /// Run a single judgment call through the tdm-runtime engine.
    ///
    /// Reads a DecisionRequest from FILE (or stdin when FILE is `-` or
    /// omitted), routes it through the runtime (provider registry,
    /// exact-hash cache, retry/circuit, audit) and prints the DecisionResult
    /// JSON to stdout. Cache hits are transparent: the result is identical,
    /// only `tdmm logs` shows the `cached` flag. Errors go to stderr.
    /// Exit codes: 0 ok; 2 caller-side failure (malformed JSON, validation,
    /// provider client error); 1 anything else.
    Call {
        /// DecisionRequest JSON file; `-` (default) reads stdin.
        file: Option<PathBuf>,
        /// Provider to judge with. Default: `[defaults].provider` (env
        /// `TDM_PROVIDER` overrides); on a bare installation `jev` when an
        /// API key env var is set, otherwise `mock`.
        #[arg(long, value_enum)]
        provider: Option<ProviderArg>,
        /// Emit compact single-line JSON instead of pretty-printed.
        #[arg(long)]
        compact: bool,
    },
    /// Set the default provider (`[defaults].provider` in config.toml).
    ///
    /// Edits the file with layout and comments preserved. Fails when the
    /// provider is not defined under `[provider.*]`.
    Use {
        /// Provider registry name, e.g. `jev` or `mock`.
        provider: String,
    },
    /// Manage provider API keys in auth.toml.
    ///
    /// Key values are only ever displayed redacted (first 4 characters +
    /// ellipsis); the full value is never printed.
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
    /// Check config files, stored keys, provider health, and the audit db.
    ///
    /// Exits 0 when the default provider is healthy, 1 otherwise. With
    /// --json, prints the full report as a JSON object.
    Doctor {
        /// Emit the report as JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
    /// Show recent audit rows (newest first).
    Logs {
        /// Only rows for this session id.
        #[arg(long)]
        session: Option<String>,
        /// Only rows for this harness.
        #[arg(long)]
        harness: Option<String>,
        /// Only rows for this provider (registry name).
        #[arg(long)]
        provider: Option<String>,
        /// Maximum number of rows.
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// Emit raw rows as a JSON array instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Aggregate audit stats per (provider, harness).
    Stats {
        /// Emit raw rows as a JSON array instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Manage the exact-hash response cache.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Locate or validate the tdm config files.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

/// `tdmm keys` subcommands.
#[derive(Debug, Subcommand)]
pub enum KeysCommand {
    /// Store the API key for a provider in auth.toml.
    Set {
        /// Provider registry name (auth.toml table).
        provider: String,
        /// API key passed directly — leaks into shell history; --stdin is
        /// preferred and a warning is printed when this is used.
        key: Option<String>,
        /// Read the API key from stdin instead of an argument.
        #[arg(long)]
        stdin: bool,
    },
    /// List stored keys (values redacted).
    List,
    /// Remove the stored key for a provider.
    Rm {
        /// Provider registry name (auth.toml table).
        provider: String,
    },
}

/// `tdmm config` subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the resolved config.toml / auth.toml / audit-db paths.
    Path,
    /// Parse config.toml and auth.toml and report errors.
    Validate,
}

/// `tdmm cache` subcommands.
#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// Evict cached results (all of them, or one provider's rows) and print
    /// how many rows were removed.
    ///
    /// The cache key covers the config-time model only, so a silent upstream
    /// model upgrade does not invalidate old entries — `cache clear` is the
    /// deliberate eviction lever.
    Clear {
        /// Only evict rows for this provider (registry name).
        #[arg(long)]
        provider: Option<String>,
    },
}

/// Explicit provider selection for `tdmm call`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ProviderArg {
    /// TypeSafe System One (requires an API key env var).
    Jev,
    /// Deterministic offline mock.
    Mock,
}

impl ProviderArg {
    /// Registry name of this provider choice.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jev => "jev",
            Self::Mock => "mock",
        }
    }
}
