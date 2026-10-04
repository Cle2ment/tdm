//! `tdmm config` — path reporting and validation (plan §6/§7).

use tdm_runtime::{AuditConfig, TdmConfig};
use toml_edit::DocumentMut;

use crate::cli::ConfigCommand;
use crate::util::{config_dir, expand_tilde};
use crate::{EXIT_FAILURE, EXIT_OK};

/// Runs `tdmm config <path|validate>`; returns the process exit code.
pub fn run(command: ConfigCommand) -> u8 {
    let result = match command {
        ConfigCommand::Path => print_paths(),
        ConfigCommand::Validate => validate(),
    };
    match result {
        Ok(()) => EXIT_OK,
        Err(error) => {
            eprintln!("error: {error:#}");
            EXIT_FAILURE
        }
    }
}

/// Prints the resolved config / auth / audit-db paths (read-only; nothing
/// is created).
///
/// # Errors
/// When the config directory cannot be resolved.
fn print_paths() -> anyhow::Result<()> {
    let dir = config_dir()?;
    let audit = TdmConfig::load()
        .map(|config| config.audit.path)
        .unwrap_or_else(|_| AuditConfig::default().path);
    println!("config:   {}", dir.join("config.toml").display());
    println!("auth:     {}", dir.join("auth.toml").display());
    println!("audit db: {}", expand_tilde(&audit).display());
    Ok(())
}

/// Parses both files and reports per-file status on stdout; a missing file
/// counts as valid (built-in defaults / no keys apply).
///
/// # Errors
/// When the config directory cannot be resolved, or when either file fails
/// to parse (after the status lines were printed).
fn validate() -> anyhow::Result<()> {
    let dir = config_dir()?;
    let mut ok = true;

    let config_path = dir.join("config.toml");
    match std::fs::read_to_string(&config_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "config: missing ({}) — built-in defaults apply",
                config_path.display()
            );
        }
        Err(error) => {
            ok = false;
            println!("config: ERROR ({}) — {error}", config_path.display());
        }
        Ok(text) => match TdmConfig::from_toml_str(&text) {
            Ok(_) => println!("config: ok ({})", config_path.display()),
            Err(error) => {
                ok = false;
                println!("config: ERROR ({}) — {error}", config_path.display());
            }
        },
    }

    let auth_path = dir.join("auth.toml");
    match std::fs::read_to_string(&auth_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("auth: missing ({}) — no stored keys", auth_path.display());
        }
        Err(error) => {
            ok = false;
            println!("auth: ERROR ({}) — {error}", auth_path.display());
        }
        Ok(text) => match text.parse::<DocumentMut>() {
            Ok(_) => println!("auth: ok ({})", auth_path.display()),
            Err(error) => {
                ok = false;
                println!("auth: ERROR ({}) — {error}", auth_path.display());
            }
        },
    }

    if !ok {
        anyhow::bail!("tdm config is invalid; fix the errors above");
    }
    Ok(())
}
