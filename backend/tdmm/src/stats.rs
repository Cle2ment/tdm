//! `tdmm stats` — per (provider, harness) audit aggregates (plan §7).

use tdm_runtime::StatsRow;

use crate::util::{open_query_runtime, print_table};
use crate::{EXIT_FAILURE, EXIT_OK};

/// Runs `tdmm stats`; returns the process exit code.
pub fn run(json: bool) -> u8 {
    match stats(json) {
        Ok(()) => EXIT_OK,
        Err(error) => {
            eprintln!("error: {error:#}");
            EXIT_FAILURE
        }
    }
}

/// Aggregates and renders the audit stats.
///
/// # Errors
/// Config resolution failures or database errors from the runtime.
fn stats(json: bool) -> anyhow::Result<()> {
    let runtime = open_query_runtime()?;
    let rows = runtime.stats()?;

    if json {
        let values: Vec<serde_json::Value> = rows.iter().map(row_json).collect();
        println!("{}", serde_json::to_string_pretty(&values)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("no audit rows");
        return Ok(());
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.provider.clone(),
                row.harness.clone(),
                row.calls.to_string(),
                row.cache_hits.to_string(),
                row.billable_input_tokens.to_string(),
                row.billable_output_tokens.to_string(),
                row.input_tokens.to_string(),
                row.output_tokens.to_string(),
                format!("{:.1}", row.avg_latency_ms),
            ]
        })
        .collect();
    print_table(
        &[
            "provider",
            "harness",
            "calls",
            "cache_hits",
            "billable_in",
            "billable_out",
            "total_in",
            "total_out",
            "avg_latency_ms",
        ],
        &table,
    );
    Ok(())
}

/// One stats row as a JSON object (camelCase wire conventions).
fn row_json(row: &StatsRow) -> serde_json::Value {
    serde_json::json!({
        "provider": row.provider,
        "harness": row.harness,
        "calls": row.calls,
        "cacheHits": row.cache_hits,
        "billableInputTokens": row.billable_input_tokens,
        "billableOutputTokens": row.billable_output_tokens,
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "avgLatencyMs": row.avg_latency_ms,
    })
}
