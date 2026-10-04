//! `tdmm keys` — auth.toml management (plan §7).
//!
//! Writes go through `toml_edit` so comments and layout survive. Key values
//! are only ever displayed redacted (first 4 characters + ellipsis); the
//! full value is never printed to stdout or stderr.

use std::io::Read;

use anyhow::{Context, bail};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::cli::KeysCommand;
use crate::util::{config_paths, redact};
use crate::{EXIT_FAILURE, EXIT_OK};

/// Runs `tdmm keys <set|list|rm>`; returns the process exit code.
pub fn run(command: KeysCommand) -> u8 {
    let result = match command {
        KeysCommand::Set {
            provider,
            key,
            stdin,
        } => set(&provider, key.as_deref(), stdin),
        KeysCommand::List => list(),
        KeysCommand::Rm { provider } => remove(&provider),
    };
    match result {
        Ok(()) => EXIT_OK,
        Err(error) => {
            eprintln!("error: {error:#}");
            EXIT_FAILURE
        }
    }
}

fn auth_path() -> anyhow::Result<std::path::PathBuf> {
    Ok(config_paths()?.1)
}

/// Loads auth.toml, or a fresh document when the file does not exist yet.
fn load_or_new_auth() -> anyhow::Result<DocumentMut> {
    let path = auth_path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => text
            .parse::<DocumentMut>()
            .with_context(|| format!("{} is not valid TOML", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(error) => Err(anyhow::Error::new(error))
            .with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Stores `key` under `[provider].api_key`, reading it from stdin when
/// `--stdin` is given. A positional key works but warns: CLI arguments leak
/// into shell history.
fn set(provider: &str, key: Option<&str>, stdin: bool) -> anyhow::Result<()> {
    let key = match (stdin, key) {
        (true, Some(_)) => {
            bail!("pass the key either via --stdin or as an argument, not both");
        }
        (true, None) => {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .context("cannot read stdin")?;
            buffer.trim().to_owned()
        }
        (false, Some(key)) => {
            eprintln!(
                "warning: the key was passed as a CLI argument and may leak into shell \
                 history; prefer `tdmm keys set {provider} --stdin`"
            );
            key.to_owned()
        }
        (false, None) => {
            bail!("no key given: use --stdin (recommended) or pass the key as an argument");
        }
    };
    if key.is_empty() {
        bail!("api key must not be empty");
    }
    let shown = redact(&key);

    let path = auth_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let mut doc = load_or_new_auth()?;
    if doc.get(provider).is_some_and(|item| !item.is_table()) {
        bail!("[{provider}] in {} is not a table", path.display());
    }
    // An explicit Table renders with a `[provider]` header; the default
    // nested assignment would produce an inline `provider = { … }`.
    if doc.get(provider).is_none() {
        doc[provider] = Item::Table(Table::new());
    }
    doc[provider]["api_key"] = value(key);
    std::fs::write(&path, doc.to_string())
        .with_context(|| format!("cannot write {}", path.display()))?;
    println!(
        "api key for {provider} saved to {path} (stored: {shown})",
        path = path.display()
    );
    Ok(())
}

/// Lists stored keys, values redacted.
fn list() -> anyhow::Result<()> {
    let path = auth_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("no keys stored ({})", path.display());
            return Ok(());
        }
        Err(error) => {
            return Err(anyhow::Error::new(error))
                .with_context(|| format!("cannot read {}", path.display()));
        }
    };
    let doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;

    let mut shown = 0;
    for (name, item) in doc.as_table().iter() {
        shown += 1;
        match item.get("api_key").and_then(|entry| entry.as_str()) {
            Some(key) if !key.is_empty() => println!("{name}: {}", redact(key)),
            Some(_) => println!("{name}: (empty)"),
            None => println!("{name}: (no api_key entry)"),
        }
    }
    if shown == 0 {
        println!("no keys stored ({})", path.display());
    }
    Ok(())
}

/// Removes the `[provider]` table from auth.toml.
fn remove(provider: &str) -> anyhow::Result<()> {
    let path = auth_path()?;
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "cannot read {} (no key stored for {provider})",
            path.display()
        )
    })?;
    let mut doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;
    if doc.as_table_mut().remove(provider).is_none() {
        bail!("no key stored for {provider} in {}", path.display());
    }
    std::fs::write(&path, doc.to_string())
        .with_context(|| format!("cannot write {}", path.display()))?;
    println!("api key for {provider} removed ({})", path.display());
    Ok(())
}
