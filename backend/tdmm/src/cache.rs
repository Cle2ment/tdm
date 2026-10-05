//! `tdmm cache` — exact-hash response-cache management.
//!
//! `cache clear` evicts rows from the runtime's cache table (optionally
//! scoped to one provider) and prints how many rows were removed. It is the
//! operational lever for the cache-key limitation documented on
//! `tdm_runtime::cache_key`: the key covers the config-time model only, so a
//! silent upstream model upgrade never invalidates old entries on its own.

use tdm_runtime::Runtime;

use crate::cli::CacheCommand;
use crate::util::open_query_runtime;
use crate::{EXIT_FAILURE, EXIT_OK};

/// Runs `tdmm cache clear`; returns the process exit code.
pub fn run(command: CacheCommand) -> u8 {
    let CacheCommand::Clear { provider } = command;
    match clear(provider.as_deref()) {
        Ok(removed) => {
            println!("removed {removed} cache row(s)");
            EXIT_OK
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            EXIT_FAILURE
        }
    }
}

/// Evicts cache rows and returns the removed count.
///
/// # Errors
/// Config resolution failures or database errors from the runtime.
fn clear(provider: Option<&str>) -> anyhow::Result<u64> {
    let runtime: Runtime = open_query_runtime()?;
    Ok(runtime.cache_clear(provider)?)
}
