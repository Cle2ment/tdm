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
    result_json TEXT NOT NULL,
    model TEXT
);
";

/// Deterministic exact-hash cache key: SHA-256 hex over `provider_id`, a NUL
/// byte, the model id (empty when unknown), a NUL byte, and the canonical
/// JSON serialization of `{ state, questions }`. `serde_json` serializes
/// maps as ordered `BTreeMap`s, so the digest is stable across JSON key
/// orders.
///
/// Known limitation (M1 review SHOULD-FIX 2): the key covers the
/// *config-time* model id (e.g. `"jev-latest"`). The model the provider
/// actually answered with — `result.provider.model` (e.g. `"jev-1.13.0"`)
/// — is only known after a call, so it cannot participate in the key. The
/// wire model is stored in the cache table's `model` column for
/// observability, but a silent upstream model upgrade does **not** invalidate
/// old entries; the operational lever is [`Store::cache_clear`] (exposed as
/// `tdmm cache clear`).
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
    /// Sum of input tokens over all rows, cache hits included (each hit
    /// replays the usage recorded by the original call).
    pub input_tokens: u64,
    /// Sum of output tokens over all rows, cache hits included.
    pub output_tokens: u64,
    /// Sum of input tokens over uncached rows only — the cost-bearing
    /// figure: cache hits do not consume API tokens.
    pub billable_input_tokens: u64,
    /// Sum of output tokens over uncached rows only.
    pub billable_output_tokens: u64,
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
        Self::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Brings databases created by older versions up to the current schema.
    ///
    /// `CREATE TABLE IF NOT EXISTS` cannot alter tables that already exist,
    /// so the cache table's `model` column (added in M2 to record the wire
    /// model behind each entry) is detected via `PRAGMA table_info` and
    /// appended with `ALTER TABLE` when missing. Pre-M2 `audit.db` files keep
    /// working; the migration is idempotent and never touches row data.
    ///
    /// # Errors
    /// [`RuntimeError::Db`] on SQLite failure.
    fn migrate(conn: &Connection) -> Result<(), RuntimeError> {
        let has_model = conn
            .prepare("PRAGMA table_info(cache)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .any(|name| name.as_deref() == Ok("model"));
        if !has_model {
            conn.execute("ALTER TABLE cache ADD COLUMN model TEXT", [])?;
        }
        Ok(())
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

    /// Upserts `result` under `key`, recording the wire model that produced
    /// it (`result.provider.model`) for observability.
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
        let wire_model = result.provider.model.as_deref();
        let conn = self
            .conn
            .lock()
            .expect("cache db mutex must not be poisoned");
        conn.execute(
            "INSERT INTO cache (key, provider, created_at, result_json, model)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key) DO UPDATE SET
                provider = ?2, created_at = ?3, result_json = ?4, model = ?5",
            params![key, provider, now_secs, json, wire_model],
        )?;
        Ok(())
    }

    /// Deletes cache rows — every row when `provider` is `None`, else only
    /// that provider's rows — returning how many rows were removed.
    ///
    /// This is the operational lever for the cache-key limitation documented
    /// on [`cache_key`]: a silent upstream model upgrade does not invalidate
    /// old entries, so eviction is deliberate (`tdmm cache clear`).
    ///
    /// # Errors
    /// [`RuntimeError::Db`] on SQLite failure.
    pub(crate) fn cache_clear(&self, provider: Option<&str>) -> Result<u64, RuntimeError> {
        let conn = self
            .conn
            .lock()
            .expect("cache db mutex must not be poisoned");
        let removed = match provider {
            Some(provider) => {
                conn.execute("DELETE FROM cache WHERE provider = ?1", params![provider])?
            }
            None => conn.execute("DELETE FROM cache", [])?,
        };
        Ok(u64::try_from(removed).unwrap_or(u64::MAX))
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
                    COALESCE(SUM(CASE WHEN cached = 0 THEN input_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN cached = 0 THEN output_tokens ELSE 0 END), 0),
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
                billable_input_tokens: row.get::<_, i64>(6)?.max(0) as u64,
                billable_output_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                avg_latency_ms: row.get(8)?,
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

    /// A hand-built audit row for stats seeding.
    fn audit_row<'a>(
        harness: &'a str,
        provider: &'a str,
        cached: bool,
        input_tokens: u64,
        output_tokens: u64,
    ) -> AuditInsert<'a> {
        AuditInsert {
            harness,
            session_id: "s1",
            provider,
            model: None,
            request_hash: "h",
            request_json: "{}",
            answers_json: None,
            error: None,
            latency_ms: 10,
            input_tokens,
            output_tokens,
            cached,
        }
    }

    #[test]
    fn stats_billable_tokens_exclude_cache_hits() {
        let store = Store::open_in_memory().expect("opens");
        store
            .audit_insert(audit_row("t", "mock", false, 10, 5), 100)
            .expect("insert");
        store
            .audit_insert(audit_row("t", "mock", true, 10, 5), 101)
            .expect("insert");
        store
            .audit_insert(audit_row("t", "mock", false, 7, 2), 102)
            .expect("insert");
        store
            .audit_insert(audit_row("t", "jev", true, 50, 25), 103)
            .expect("insert");

        let stats = store.stats().expect("stats");
        let mock = stats.iter().find(|s| s.provider == "mock").unwrap();
        assert_eq!(mock.calls, 3);
        assert_eq!(mock.cache_hits, 1);
        assert_eq!(mock.input_tokens, 27, "totals include the cached replay");
        assert_eq!(mock.output_tokens, 12);
        assert_eq!(mock.billable_input_tokens, 17, "billable excludes the hit");
        assert_eq!(mock.billable_output_tokens, 7);

        let jev = stats.iter().find(|s| s.provider == "jev").unwrap();
        assert_eq!(jev.input_tokens, 50);
        assert_eq!(
            jev.billable_input_tokens, 0,
            "all-hit groups are not billable"
        );
        assert_eq!(jev.billable_output_tokens, 0);
    }

    #[test]
    fn cache_clear_scopes_by_provider_and_counts_rows() {
        let store = Store::open_in_memory().expect("opens");
        let result_for = |id: &str| DecisionResult {
            answers: vec![],
            usage: tdm_core::Usage {
                input_tokens: 1,
                output_tokens: 1,
            },
            provider: tdm_core::ProviderMeta {
                id: id.to_owned(),
                model: Some("m".to_owned()),
            },
            latency_ms: 1,
        };
        let key_a = cache_key(
            "mock",
            Some("m"),
            &noul_request(serde_json::json!({"a": 1})),
        );
        let key_b = cache_key(
            "mock",
            Some("m"),
            &noul_request(serde_json::json!({"a": 2})),
        );
        let key_c = cache_key("jev", Some("m"), &noul_request(serde_json::json!({"a": 1})));
        store
            .cache_put(&key_a, "mock", 100, &result_for("mock"))
            .expect("put");
        store
            .cache_put(&key_b, "mock", 100, &result_for("mock"))
            .expect("put");
        store
            .cache_put(&key_c, "jev", 100, &result_for("jev"))
            .expect("put");

        assert_eq!(
            store.cache_clear(Some("mock")).expect("clear"),
            2,
            "only the mock rows are removed"
        );
        assert!(store.cache_get(&key_a, 3600, 150).expect("get").is_none());
        assert!(store.cache_get(&key_b, 3600, 150).expect("get").is_none());
        assert!(
            store.cache_get(&key_c, 3600, 150).expect("get").is_some(),
            "other providers' rows survive a scoped clear"
        );

        assert_eq!(
            store.cache_clear(None).expect("clear"),
            1,
            "clear-all counts what is left"
        );
        assert_eq!(store.cache_clear(None).expect("clear"), 0, "idempotent");
    }

    #[test]
    fn migration_adds_model_column_to_pre_m2_cache_databases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.db");
        let legacy_result = DecisionResult {
            answers: vec![],
            usage: tdm_core::Usage {
                input_tokens: 3,
                output_tokens: 1,
            },
            provider: tdm_core::ProviderMeta {
                id: "mock".to_owned(),
                model: Some("mock-1".to_owned()),
            },
            latency_ms: 4,
        };

        // Simulate a pre-M2 database: a cache table without the model column.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE cache (
                     key TEXT PRIMARY KEY,
                     provider TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     result_json TEXT NOT NULL
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO cache (key, provider, created_at, result_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "legacy",
                    "mock",
                    100,
                    serde_json::to_string(&legacy_result).unwrap()
                ],
            )
            .unwrap();
        }

        let store = Store::open(&path).expect("a pre-M2 database opens and migrates");

        // Seeded rows survive the migration and still read back.
        let hit = store.cache_get("legacy", 3600, 150).expect("get");
        assert_eq!(hit, Some(legacy_result.clone()));

        // cache_put writes the model column — proof the migration added it
        // (the INSERT would fail with "no such column" otherwise).
        let key = cache_key(
            "mock",
            Some("mock-1"),
            &noul_request(serde_json::json!({"a": 1})),
        );
        store
            .cache_put(&key, "mock", 100, &legacy_result)
            .expect("put into the migrated table");

        // The wire model is observable in the row.
        let conn = Connection::open(&path).unwrap();
        let stored_model: Option<String> = conn
            .query_row(
                "SELECT model FROM cache WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored_model.as_deref(),
            Some("mock-1"),
            "wire model recorded at put time"
        );

        // Reopening an already-migrated database is idempotent.
        drop(conn);
        drop(store);
        let reopened = Store::open(&path).expect("reopen after migration");
        assert!(reopened.cache_get(&key, 3600, 150).expect("get").is_some());
    }
}
