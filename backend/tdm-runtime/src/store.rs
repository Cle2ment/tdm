//! SQLite-backed state: the audit log and the exact-hash response cache
//! (ADR-0005: SQLite stores state, configuration stays in TOML files).
//!
//! Both tables live in one database whose path comes from `[audit].path`.
//! All access goes through a single mutex-guarded connection — the operations
//! are short, local SQLite statements, so blocking the async runtime briefly
//! is acceptable for M1.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tdm_core::{DecisionRequest, DecisionResult, Question};

use crate::error::RuntimeError;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS audit (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    harness TEXT NOT NULL,
    session_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT,
    request_hash TEXT NOT NULL,
    request_json TEXT NOT NULL,
    answers_json TEXT,
    error TEXT,
    latency_ms INTEGER NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cached INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS audit_ts_idx ON audit(ts);
CREATE INDEX IF NOT EXISTS audit_session_idx ON audit(session_id);
CREATE TABLE IF NOT EXISTS cache (
    key TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    result_json TEXT NOT NULL
);
";

/// Deterministic exact-hash cache key: SHA-256 hex over `provider_id`, a NUL
/// byte, the model id (empty when unknown), a NUL byte, and the canonical
/// JSON serialization of `{ state, questions }`. `serde_json` serializes
/// maps as ordered `BTreeMap`s, so the digest is stable across JSON key
/// orders.
#[must_use]
pub fn cache_key(provider_id: &str, model: Option<&str>, request: &DecisionRequest) -> String {
    let body = CacheKeyBody {
        state: &request.state,
        questions: &request.questions,
    };
    let canonical = serde_json::to_vec(&body).unwrap_or_default();

    let mut hasher = Sha256::new();
    hasher.update(provider_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(model.unwrap_or("").as_bytes());
    hasher.update([0u8]);
    hasher.update(&canonical);
    format!("{:x}", hasher.finalize())
}

#[derive(Serialize)]
struct CacheKeyBody<'a> {
    state: &'a serde_json::Value,
    questions: &'a [Question],
}

/// One audit row as written by the runtime (before `id`/`ts` are assigned).
pub(crate) struct AuditInsert<'a> {
    pub harness: &'a str,
    pub session_id: &'a str,
    pub provider: &'a str,
    pub model: Option<&'a str>,
    pub request_hash: &'a str,
    pub request_json: &'a str,
    pub answers_json: Option<&'a str>,
    pub error: Option<&'a str>,
    pub latency_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached: bool,
}

/// A stored audit row (query result).
#[derive(Debug, Clone, PartialEq)]
pub struct AuditRow {
    /// Monotonic row id.
    pub id: i64,
    /// Insertion time, unix seconds.
    pub ts: i64,
    /// Harness that issued the call.
    pub harness: String,
    /// Harness-side session id.
    pub session_id: String,
    /// Registry name of the routed provider.
    pub provider: String,
    /// Model that answered, when known.
    pub model: Option<String>,
    /// Exact-hash cache key of the request.
    pub request_hash: String,
    /// Full request JSON (camelCase wire format).
    pub request_json: String,
    /// Answer JSON, when a result was produced (cache hit or success).
    pub answers_json: Option<String>,
    /// Error rendering, when the call failed.
    pub error: Option<String>,
    /// End-to-end wall time of the judgment call (incl. retries), ms.
    pub latency_ms: u64,
    /// Token usage attributed to the call (from the result, cached or not).
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Whether the answer came from the cache.
    pub cached: bool,
}

/// Optional filters for [`crate::Runtime::audit_recent`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditFilter {
    /// Keep rows for this session only.
    pub session_id: Option<String>,
    /// Keep rows for this harness only.
    pub harness: Option<String>,
    /// Keep rows for this provider (registry name) only.
    pub provider: Option<String>,
}

/// Per (`provider`, `harness`) aggregate over the audit log.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsRow {
    /// Registry name of the provider.
    pub provider: String,
    /// Harness that issued the calls.
    pub harness: String,
    /// Total judgment calls (success + cache hits + failures).
    pub calls: u64,
    /// How many of those were served from the cache.
    pub cache_hits: u64,
    /// Sum of input tokens over all rows (cache hits included).
    pub input_tokens: u64,
    /// Sum of output tokens over all rows (cache hits included).
    pub output_tokens: u64,
    /// Mean end-to-end latency in milliseconds.
    pub avg_latency_ms: f64,
}

/// The shared audit + cache database.
#[derive(Debug)]
pub(crate) struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Opens (creating directories, tables, and WAL mode as needed) the
    /// database at `path`. The special value `:memory:` opens an in-memory
    /// database.
    ///
    /// # Errors
    /// [`RuntimeError::Io`] / [`RuntimeError::Db`] on filesystem or SQLite
    /// failures.
    pub(crate) fn open(path: &Path) -> Result<Self, RuntimeError> {
        if path.as_os_str() == ":memory:" {
            return Self::open_in_memory();
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    /// Opens a throwaway in-memory database.
    ///
    /// # Errors
    /// [`RuntimeError::Db`] on SQLite failure.
    pub(crate) fn open_in_memory() -> Result<Self, RuntimeError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, RuntimeError> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Returns the cached result for `key` when it exists and is younger than
    /// `ttl_secs` (relative to `now_secs`); expired entries are deleted.
    ///
    /// # Errors
    /// [`RuntimeError::Db`] on SQLite failure, [`RuntimeError::Json`] when a
    /// stored payload is corrupt.
    pub(crate) fn cache_get(
        &self,
        key: &str,
        ttl_secs: u64,
        now_secs: i64,
    ) -> Result<Option<DecisionResult>, RuntimeError> {
        let conn = self
            .conn
            .lock()
            .expect("cache db mutex must not be poisoned");
        let row = conn
            .query_row(
                "SELECT created_at, result_json FROM cache WHERE key = ?1",
                params![key],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((created_at, json)) = row else {
            return Ok(None);
        };
        let ttl = i64::try_from(ttl_secs).unwrap_or(i64::MAX);
        if now_secs - created_at >= ttl {
            conn.execute("DELETE FROM cache WHERE key = ?1", params![key])?;
            return Ok(None);
        }
        let result = serde_json::from_str(&json)
            .map_err(|error| RuntimeError::other(format!("cached result is corrupt: {error}")))?;
        Ok(Some(result))
    }

    /// Upserts `result` under `key`.
    ///
    /// # Errors
    /// [`RuntimeError`]: serialization or SQLite failure.
    pub(crate) fn cache_put(
        &self,
        key: &str,
        provider: &str,
        now_secs: i64,
        result: &DecisionResult,
    ) -> Result<(), RuntimeError> {
        let json = serde_json::to_string(result)?;
        let conn = self
            .conn
            .lock()
            .expect("cache db mutex must not be poisoned");
        conn.execute(
            "INSERT INTO cache (key, provider, created_at, result_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET
                provider = ?2, created_at = ?3, result_json = ?4",
            params![key, provider, now_secs, json],
        )?;
        Ok(())
    }

    /// Appends one audit row with `ts` as its unix-second timestamp.
    ///
    /// # Errors
    /// [`RuntimeError::Db`] on SQLite failure.
    pub(crate) fn audit_insert(&self, row: AuditInsert<'_>, ts: i64) -> Result<(), RuntimeError> {
        let conn = self
            .conn
            .lock()
            .expect("audit db mutex must not be poisoned");
        conn.execute(
            "INSERT INTO audit (
                ts, harness, session_id, provider, model, request_hash,
                request_json, answers_json, error, latency_ms,
                input_tokens, output_tokens, cached
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                ts,
                row.harness,
                row.session_id,
                row.provider,
                row.model,
                row.request_hash,
                row.request_json,
                row.answers_json,
                row.error,
                i64::try_from(row.latency_ms).unwrap_or(i64::MAX),
                i64::try_from(row.input_tokens).unwrap_or(i64::MAX),
                i64::try_from(row.output_tokens).unwrap_or(i64::MAX),
                row.cached,
            ],
        )?;
        Ok(())
    }

    /// Most recent rows matching `filter`, newest first.
    ///
    /// # Errors
    /// [`RuntimeError::Db`] on SQLite failure.
    pub(crate) fn audit_recent(
        &self,
        filter: AuditFilter,
        limit: u32,
    ) -> Result<Vec<AuditRow>, RuntimeError> {
        let conn = self
            .conn
            .lock()
            .expect("audit db mutex must not be poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, ts, harness, session_id, provider, model, request_hash,
                    request_json, answers_json, error, latency_ms,
                    input_tokens, output_tokens, cached
             FROM audit
             WHERE (?1 IS NULL OR session_id = ?1)
               AND (?2 IS NULL OR harness = ?2)
               AND (?3 IS NULL OR provider = ?3)
             ORDER BY id DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                filter.session_id,
                filter.harness,
                filter.provider,
                i64::from(limit)
            ],
            |row| {
                Ok(AuditRow {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    harness: row.get(2)?,
                    session_id: row.get(3)?,
                    provider: row.get(4)?,
                    model: row.get(5)?,
                    request_hash: row.get(6)?,
                    request_json: row.get(7)?,
                    answers_json: row.get(8)?,
                    error: row.get(9)?,
                    latency_ms: row.get::<_, i64>(10)?.max(0) as u64,
                    input_tokens: row.get::<_, i64>(11)?.max(0) as u64,
                    output_tokens: row.get::<_, i64>(12)?.max(0) as u64,
                    cached: row.get::<_, i64>(13)? != 0,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(RuntimeError::from)
    }

    /// Aggregates the audit log per (`provider`, `harness`).
    ///
    /// # Errors
    /// [`RuntimeError::Db`] on SQLite failure.
    pub(crate) fn stats(&self) -> Result<Vec<StatsRow>, RuntimeError> {
        let conn = self
            .conn
            .lock()
            .expect("audit db mutex must not be poisoned");
        let mut stmt = conn.prepare(
            "SELECT provider, harness, COUNT(*), COALESCE(SUM(cached), 0),
                    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                    COALESCE(AVG(latency_ms), 0.0)
             FROM audit
             GROUP BY provider, harness
             ORDER BY provider, harness",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StatsRow {
                provider: row.get(0)?,
                harness: row.get(1)?,
                calls: row.get::<_, i64>(2)?.max(0) as u64,
                cache_hits: row.get::<_, i64>(3)?.max(0) as u64,
                input_tokens: row.get::<_, i64>(4)?.max(0) as u64,
                output_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                avg_latency_ms: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(RuntimeError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noul_request(state: serde_json::Value) -> DecisionRequest {
        DecisionRequest {
            state,
            questions: vec![Question {
                id: "q1".to_owned(),
                instructions: "is it safe?".to_owned(),
                primitive: tdm_core::Primitive::Noul,
            }],
        }
    }

    #[test]
    fn cache_key_ignores_json_object_order() {
        let a = cache_key(
            "jev",
            Some("m"),
            &noul_request(serde_json::json!({"a": 1, "b": {"x": 1, "y": 2}})),
        );
        let b = cache_key(
            "jev",
            Some("m"),
            &noul_request(serde_json::json!({"b": {"y": 2, "x": 1}, "a": 1})),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn cache_key_varies_by_provider_model_and_payload() {
        let base = noul_request(serde_json::json!({"a": 1}));
        let other_provider = cache_key("mock", Some("m"), &base);
        let other_model = cache_key("jev", Some("m2"), &base);
        let other_payload = cache_key("jev", Some("m"), &noul_request(serde_json::json!({"a": 2})));
        let reference = cache_key("jev", Some("m"), &base);
        assert_ne!(reference, other_provider);
        assert_ne!(reference, other_model);
        assert_ne!(reference, other_payload);
        assert_eq!(reference.len(), 64, "sha256 hex digest");
    }

    #[test]
    fn in_memory_store_roundtrips_rows() {
        let store = Store::open_in_memory().expect("opens");
        let req = noul_request(serde_json::json!({"a": 1}));
        let key = cache_key("mock", Some("mock-1"), &req);

        assert!(store.cache_get(&key, 3600, 100).expect("get").is_none());

        let result = DecisionResult {
            answers: vec![],
            usage: tdm_core::Usage {
                input_tokens: 7,
                output_tokens: 3,
            },
            provider: tdm_core::ProviderMeta {
                id: "mock".to_owned(),
                model: Some("mock-1".to_owned()),
            },
            latency_ms: 5,
        };
        store.cache_put(&key, "mock", 100, &result).expect("put");
        let hit = store.cache_get(&key, 3600, 150).expect("get");
        assert_eq!(hit, Some(result.clone()));

        // TTL elapsed -> miss and eviction.
        assert!(store.cache_get(&key, 10, 200).expect("get").is_none());
        store.cache_put(&key, "mock", 100, &result).expect("re-put");
        assert!(store.cache_get(&key, 10, 200).expect("get").is_none());
        store.cache_put(&key, "mock", 100, &result).expect("re-put");
        assert!(store.cache_get(&key, 10, 201).expect("get").is_none());
    }
}
