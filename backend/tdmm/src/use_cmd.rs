//! `tdmm use` — switch `[defaults].provider` in config.toml (plan §7).
//!
//! The edit goes through `toml_edit` so comments and layout survive; the
//! provider must already be defined under `[provider.*]`.

use anyhow::{Context, bail};
use toml_edit::{DocumentMut, Item, Table, Value};

use crate::util::config_paths;
use crate::{EXIT_FAILURE, EXIT_OK};

/// Runs `tdmm use <provider>`; returns the process exit code.
pub fn run(provider: &str) -> u8 {
    match set_default(provider) {
        Ok(()) => EXIT_OK,
        Err(error) => {
            eprintln!("error: {error:#}");
            EXIT_FAILURE
        }
    }
}

/// Rewrites `[defaults].provider`, preserving the rest of the file.
///
/// # Errors
/// Missing / unreadable / invalid config.toml, an unknown provider name, or
/// a write failure.
fn set_default(provider: &str) -> anyhow::Result<()> {
    let (config_path, _) = config_paths()?;
    let text = std::fs::read_to_string(&config_path).with_context(|| {
        format!(
            "cannot read {} (run `tdmm init` first)",
            config_path.display()
        )
    })?;
    let mut doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("{} is not valid TOML", config_path.display()))?;

    let defined = doc
        .get("provider")
        .and_then(|item| item.as_table())
        .is_some_and(|table| table.contains_key(provider));
    if !defined {
        bail!(
            "provider {provider:?} is not defined under [provider.*] in {}",
            config_path.display()
        );
    }
    if doc.get("defaults").is_some_and(|item| !item.is_table()) {
        bail!(
            "[defaults] in {} is not a table; fix it by hand",
            config_path.display()
        );
    }
    // An explicit Table renders with a `[defaults]` header (see keys.rs);
    // the previous value's decor is carried over so trailing comments on
    // the line survive the rewrite.
    if doc.get("defaults").is_none() {
        doc["defaults"] = Item::Table(Table::new());
    }
    let decor = doc
        .get("defaults")
        .and_then(|defaults| defaults.get("provider"))
        .and_then(|item| item.as_value())
        .map(|value| value.decor().clone());
    let mut new_value = Value::from(provider);
    if let Some(decor) = decor {
        *new_value.decor_mut() = decor;
    }
    doc["defaults"]["provider"] = Item::Value(new_value);

    std::fs::write(&config_path, doc.to_string())
        .with_context(|| format!("cannot write {}", config_path.display()))?;
    println!(
        "default provider set to {provider} ({})",
        config_path.display()
    );
    Ok(())
}
