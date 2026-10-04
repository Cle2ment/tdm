//! Command-line surface (clap derive).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// `tdmm` — TDM management CLI (M0 ships `init` + `call`; the full surface
/// lands in M1).
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
    /// Writes skeleton files only — TOML parsing, layered resolution, and
    /// `keys` / `config` management land in M1. Idempotent: refuses to
    /// overwrite existing files unless --force is given.
    Init {
        /// Overwrite existing config.toml / auth.toml.
        #[arg(long)]
        force: bool,
    },
    /// Run a single judgment call: request JSON in, DecisionResult JSON out.
    ///
    /// Reads a DecisionRequest from FILE (or stdin when FILE is `-` or
    /// omitted), judges it through the selected provider, and prints the
    /// DecisionResult JSON to stdout. Errors go to stderr. Exit codes:
    /// 0 ok; 2 caller-side failure (malformed JSON, validation, provider
    /// client error); 1 anything else.
    Call {
        /// DecisionRequest JSON file; `-` (default) reads stdin.
        file: Option<PathBuf>,
        /// Provider to judge with. Default: `jev` when TDM_JEV_API_KEY or
        /// TYPESAFE_API_KEY is set, otherwise `mock`.
        #[arg(long, value_enum)]
        provider: Option<ProviderArg>,
        /// Emit compact single-line JSON instead of pretty-printed.
        #[arg(long)]
        compact: bool,
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
