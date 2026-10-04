//! `tdmm logs` — audit query (plan §7).
//!
//! Reads the runtime's audit log (newest first) through the same
//! config-resolved database the engine writes to.

use tdm_runtime::{AuditFilter, AuditRow};

use crate::util::{open_query_runtime, print_table, truncate};
use crate::{EXIT_FAILURE, EXIT_OK};

/// Runs `tdmm logs`; returns the process exit code.
pub fn run(
    session: Option<String>,
    harness: Option<String>,
    provider: Option<String>,
    limit: u32,
    json: bool,
) -> u8 {
    match recent(session, harness, provider, limit, json) {
        Ok(()) => EXIT_OK,
        Err(error) => {
            eprintln!("error: {error:#}");
            EXIT_FAILURE
        }
    }
}

/// Queries and renders the audit rows.
///
/// # Errors
/// Config resolution failures or database errors from the runtime.
fn recent(
    session: Option<String>,
    harness: Option<String>,
    provider: Option<String>,
    limit: u32,
    json: bool,
) -> anyhow::Result<()> {
    let runtime = open_query_runtime()?;
    let filter = AuditFilter {
        session_id: session,
        harness,
        provider,
    };
    let rows = runtime.audit_recent(filter, limit)?;

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
                row.ts.to_string(),
                row.harness.clone(),
                row.session_id.clone(),
                row.provider.clone(),
                row.model.clone().unwrap_or_else(|| "-".to_owned()),
                row.cached.to_string(),
                row.latency_ms.to_string(),
                format!("{}/{}", row.input_tokens, row.output_tokens),
                truncate(row.error.as_deref().unwrap_or_default(), 48),
            ]
        })
        .collect();
    print_table(
        &[
            "ts",
            "harness",
            "session",
            "provider",
            "model",
            "cached",
            "latency_ms",
            "tokens",
            "error",
        ],
        &table,
    );
    Ok(())
}

/// One audit row as a JSON object (camelCase wire conventions).
fn row_json(row: &AuditRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.id,
        "ts": row.ts,
        "harness": row.harness,
        "sessionId": row.session_id,
        "provider": row.provider,
        "model": row.model,
        "requestHash": row.request_hash,
        "requestJson": row.request_json,
        "answersJson": row.answers_json,
        "error": row.error,
        "latencyMs": row.latency_ms,
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cached": row.cached,
    })
}
