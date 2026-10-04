//! `tdmm` — the TDM management CLI (plan §7).
//!
//! M0 surface: [`init`] (config/auth skeleton bootstrap) and [`call`]
//! (single-shot judgment, stdin/pipe-friendly). The rest of the planned
//! surface — `use`, `keys`, `doctor`, `logs`, `stats`, `config`, `serve` —
//! lands in M1.
//!
//! Error output goes to stderr, results to stdout only (pipe-friendly). API
//! keys are consumed but never printed.

pub mod call;
pub mod cli;
pub mod config;
pub mod init;

use clap::Parser;

use crate::cli::{Cli, Command};

/// Exit code: success.
pub const EXIT_OK: u8 = 0;
/// Exit code: any failure that is not caller-side (config, network, server,
/// quality, unsupported). Never used for malformed input.
pub const EXIT_FAILURE: u8 = 1;
/// Exit code: caller-side failure — malformed request JSON, failed request
/// validation, or a provider [`tdm_core::TdmError::Client`].
pub const EXIT_CLIENT: u8 = 2;

/// Parses argv, dispatches the subcommand, and returns the process exit code.
pub async fn run() -> u8 {
    match Cli::parse().command {
        Command::Init { force } => init::run(force),
        Command::Call {
            file,
            provider,
            compact,
        } => call::run(file, provider, compact).await,
    }
}
