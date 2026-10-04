//! `tdmm` — the TDM management CLI (plan §7).
//!
//! M1 surface: [`init`] (config/auth skeleton bootstrap), [`call`]
//! (single-shot judgment through the tdm-runtime engine — registry routing,
//! exact-hash cache, retry/circuit, audit), [`use_cmd`] (default provider
//! switch, comments preserved), [`keys`] (auth.toml management; key values
//! only ever displayed redacted), [`doctor`] (config / key / provider
//! health / audit-db diagnostics), [`logs`] + [`stats`] (audit queries),
//! and [`config_cmd`] (path + validate). `serve` (JSON-RPC daemon) is a
//! later milestone.
//!
//! Error output goes to stderr, results to stdout only (pipe-friendly). API
//! keys are consumed but never printed.

pub mod call;
pub mod cli;
pub mod config;
pub mod config_cmd;
pub mod doctor;
pub mod init;
pub mod keys;
pub mod logs;
pub mod stats;
pub mod use_cmd;
pub mod util;

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
        Command::Use { provider } => use_cmd::run(&provider),
        Command::Keys { command } => keys::run(command),
        Command::Doctor { json } => doctor::run(json).await,
        Command::Logs {
            session,
            harness,
            provider,
            limit,
            json,
        } => logs::run(session, harness, provider, limit, json),
        Command::Stats { json } => stats::run(json),
        Command::Config { command } => config_cmd::run(command),
    }
}
