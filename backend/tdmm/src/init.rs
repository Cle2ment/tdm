//! `tdmm init` — write the config/auth skeleton (plan §6).

use anyhow::Context;

use crate::config;
use crate::{EXIT_FAILURE, EXIT_OK};

/// Exit code used when the skeleton cannot be written or already exists.
const INIT_FAILURE: u8 = EXIT_FAILURE;

/// Runs `tdmm init`; returns the process exit code.
pub fn run(force: bool) -> u8 {
    match init(force) {
        Ok(()) => EXIT_OK,
        Err(error) => {
            eprintln!("error: {error:#}");
            INIT_FAILURE
        }
    }
}

/// Writes both skeleton files (creating the directory when missing) and
/// prints the created paths. Without `--force`, refuses to overwrite any
/// file that already exists.
///
/// # Errors
/// IO failures, or existing files without `--force`.
fn init(force: bool) -> anyhow::Result<()> {
    let dir = config::config_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create config directory {}", dir.display()))?;

    let config_path = dir.join("config.toml");
    let auth_path = dir.join("auth.toml");

    if !force {
        let existing: Vec<String> = [&config_path, &auth_path]
            .into_iter()
            .filter(|path| path.exists())
            .map(|path| path.display().to_string())
            .collect();
        if !existing.is_empty() {
            let listed = existing.join(", ");
            anyhow::bail!(
                "refusing to overwrite existing file(s): {listed}; pass --force to overwrite"
            );
        }
    }

    std::fs::write(&config_path, config::CONFIG_TOML)
        .with_context(|| format!("cannot write {}", config_path.display()))?;
    std::fs::write(&auth_path, config::AUTH_TOML)
        .with_context(|| format!("cannot write {}", auth_path.display()))?;

    println!("created {}", config_path.display());
    println!("created {}", auth_path.display());
    Ok(())
}
