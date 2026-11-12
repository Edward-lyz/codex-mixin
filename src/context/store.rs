use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};

use super::body::{self, BodyTags};
use super::guidance::NudgeState;
use super::types::{CacheStats, DirectRef, SegmentKind, SessionSummary, Tier};

const REASONING_FINGERPRINT_VERSION: i64 = 1;
/// From this data version `messages.fingerprint` holds a SHA-256 of the
/// normalized item instead of the normalized JSON itself.
const HASHED_FINGERPRINT_VERSION: i64 = 2;
/// The archive worker limits each pass to this many sessions.
pub(crate) const MAINTENANCE_BATCH_SIZE: usize = 8;
const BODY_BATCH_BYTES: usize = 4 * 1024 * 1024;
type StoredBody = (SqlValue, i64, i64, Option<Vec<u8>>);
type StoredResponseAnchor = (StoredBody, Option<i64>, Option<i64>, Option<String>);
type ResponseAnchor = (Vec<Value>, Option<usize>, Option<String>);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    session_key TEXT PRIMARY KEY,
    revision INTEGER NOT NULL DEFAULT 0,
    next_ref INTEGER NOT NULL DEFAULT 1,
    next_block INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL,
    last_error TEXT
);
CREATE TABLE IF NOT EXISTS nudge_state (
    session_key TEXT PRIMARY KEY,
    state_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    session_key TEXT NOT NULL,
    reference TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    item_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expired_at INTEGER,
    body_codec INTEGER NOT NULL DEFAULT 0,
    body_bytes INTEGER NOT NULL DEFAULT 0,
    body_checksum BLOB,
    body_kind TEXT,
    body_tool TEXT,
    PRIMARY KEY (session_key, reference)
);
CREATE TABLE IF NOT EXISTS history (
    session_key TEXT NOT NULL,
    position INTEGER NOT NULL,
    reference TEXT NOT NULL,
    PRIMARY KEY (session_key, position)
);
CREATE TABLE IF NOT EXISTS blocks (
    session_key TEXT NOT NULL,
    block_id TEXT NOT NULL,
    tier INTEGER NOT NULL,
    topic TEXT NOT NULL,
    summary TEXT NOT NULL,
    direct_refs TEXT NOT NULL,
    full_refs TEXT NOT NULL,
    start_position INTEGER NOT NULL,
    end_position INTEGER NOT NULL,
    superseded_by TEXT,
    covered_tokens INTEGER NOT NULL DEFAULT 0,
    replacement_tokens INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (session_key, block_id)
);
CREATE TABLE IF NOT EXISTS segments (
    session_key TEXT NOT NULL,
    revision INTEGER NOT NULL,
    position INTEGER NOT NULL,
    end_position INTEGER NOT NULL,
    kind TEXT NOT NULL,
    reference TEXT NOT NULL,
    PRIMARY KEY (session_key, revision, position)
);
CREATE TABLE IF NOT EXISTS responses (
    session_key TEXT NOT NULL,
    response_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    output_json TEXT NOT NULL DEFAULT '[]',
    body_codec INTEGER NOT NULL DEFAULT 0,
    body_bytes INTEGER NOT NULL DEFAULT 0,
    body_checksum BLOB,
    created_at INTEGER NOT NULL,
    expired_at INTEGER,
    history_len INTEGER,
    history_tip TEXT,
    PRIMARY KEY (session_key, response_id)
);
CREATE TABLE IF NOT EXISTS leases (
    session_key TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    acquired_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS turns (
    session_key TEXT NOT NULL,
    at_ms INTEGER NOT NULL,
    input_tokens INTEGER NOT NULL,
    cached_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cache_metrics_available INTEGER NOT NULL DEFAULT 0,
    codex_thread_id TEXT
);
CREATE INDEX IF NOT EXISTS turns_session ON turns (session_key, at_ms);
CREATE TABLE IF NOT EXISTS maintenance (
    name TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
";

/// Stored fingerprint of one normalized history item.
pub(super) fn fingerprint_digest(normalized: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, normalized.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Cached prompt tokens are a subset of the billed input tokens.
pub(crate) fn cache_hit_percent(cached_tokens: u64, input_tokens: u64) -> Option<f64> {
    (input_tokens > 0).then(|| cached_tokens as f64 / input_tokens as f64 * 100.0)
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
pub(crate) struct SessionRow {
    pub revision: i64,
    pub next_ref: u64,
    pub next_block: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct HistoryEntry {
    pub position: i64,
    pub reference: String,
}

#[derive(Clone, Debug)]
pub(crate) struct SegmentRow {
    pub position: i64,
    pub end_position: i64,
    pub kind: SegmentKind,
    pub reference: String,
}

#[derive(Clone, Debug)]
pub(crate) struct BlockRow {
    pub id: String,
    pub tier: Tier,
    pub topic: String,
    pub summary: String,
    pub direct_refs: Vec<DirectRef>,
    pub full_refs: Vec<String>,
}

/// Billed usage of one upstream round; unknown counters are not recorded so
/// cached-token totals stay honest.
#[derive(Clone, Debug)]
pub(crate) struct TurnSample {
    pub at_ms: i64,
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub output_tokens: u64,
    pub cache_metrics_available: bool,
    pub codex_thread_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionUsage {
    pub session: String,
    pub codex_thread_id: Option<String>,
    pub revision: i64,
    pub updated_at: i64,
    pub messages: u64,
    /// Blocks created, including the ones a higher tier superseded.
    pub compressions: u64,
    /// Tokens the live blocks removed from the view.
    pub saved_tokens: u64,
    pub cache: CacheStats,
}

#[derive(Clone, Debug)]
pub(crate) struct SearchHit {
    pub kind: String,
    pub reference: String,
    pub title: String,
    pub snippet: String,
}

pub(crate) struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("create context database directory {}", parent.display())
            })?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("open context database {}", path.display()))?;
        let store = Self { connection };
        store.configure()?;
        Ok(store)
    }

    fn configure(&self) -> Result<()> {
        self.connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .context("set context busy timeout")?;
        self.connection
            .pragma_update(None, "journal_mode", "WAL")
            .context("set context journal mode")?;
        self.connection
            .pragma_update(None, "synchronous", "NORMAL")
            .context("set context synchronous mode")?;
        self.connection
            .pragma_update(None, "journal_size_limit", 8 * 1024 * 1024)?;
        self.connection
            .pragma_update(None, "wal_autocheckpoint", 256)?;
        self.connection
            .pragma_update(None, "cache_size", -256)
            .context("set context sqlite cache size")?;
        // A new database starts in the current format, so it needs neither
        // the digest upgrade, its backup, nor a full VACUUM later.
        let fresh: bool = self
            .connection
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'messages')",
                [],
                |row| row.get(0),
            )
            .context("inspect context schema")?;
        if fresh {
            self.connection
                .pragma_update(None, "auto_vacuum", "INCREMENTAL")
                .context("set context auto vacuum mode")?;
        }
        self.connection
            .execute_batch(SCHEMA)
            .context("create context schema")?;
        if fresh {
            self.connection
                .pragma_update(None, "user_version", HASHED_FINGERPRINT_VERSION)
                .context("save context data version")?;
        }
        self.upgrade_response_output()
            .context("upgrade context schema")?;
        self.upgrade_block_tokens()
            .context("upgrade context schema")?;
        let mut statement = self.connection.prepare("PRAGMA table_info(turns)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if !columns
            .iter()
            .any(|column| column == "cache_metrics_available")
        {
            // Legacy rounds have no provider/model identity. Keep billed usage,
            // but do not claim their cache counters are comparable.
            self.connection.execute("ALTER TABLE turns ADD COLUMN cache_metrics_available INTEGER NOT NULL DEFAULT 0", [])
                .context("upgrade context cache accounting")?;
        }
        if !columns.iter().any(|column| column == "codex_thread_id") {
            self.connection
                .execute("ALTER TABLE turns ADD COLUMN codex_thread_id TEXT", [])
                .context("upgrade context thread metadata")?;
        }
        self.upgrade_retention_columns()
            .context("upgrade context retention schema")?;
        self.backfill_latest_response_anchor()
            .context("backfill latest context response anchor")?;
        self.upgrade_reasoning_fingerprints()?;
        // CLI readers share this database with the gateway. Opening another
        // connection must never revoke a live turn's lease.
        self.connection
            .execute("DELETE FROM leases WHERE expires_at <= ?1", [unix_now()])
            .context("clear expired context leases")?;
        self.upgrade_hashed_fingerprints()?;
        Ok(())
    }

    /// Rewrites archived JSON fingerprints as digests. Returns whether this
    /// connection performed the upgrade.
    fn upgrade_hashed_fingerprints(&self) -> Result<bool> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .context("read context data version")?;
        if version >= HASHED_FINGERPRINT_VERSION {
            return Ok(false);
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
                .context("begin fingerprint digest upgrade")?;
        let version: i64 = transaction
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .context("read context data version")?;
        if version >= HASHED_FINGERPRINT_VERSION {
            return Ok(false);
        }
        let mut last_rowid = 0_i64;
        loop {
            let batch = {
                let mut statement = transaction
                    .prepare_cached(
                        "SELECT rowid, fingerprint FROM messages WHERE rowid > ?1 ORDER BY rowid LIMIT 512",
                    )
                    .context("prepare fingerprint digest batch")?;
                statement
                    .query_map([last_rowid], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })
                    .context("read fingerprint digest batch")?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .context("read fingerprint digest row")?
            };
            let Some((rowid, _)) = batch.last() else {
                break;
            };
            last_rowid = *rowid;
            let mut update = transaction
                .prepare_cached("UPDATE messages SET fingerprint = ?2 WHERE rowid = ?1")
                .context("prepare fingerprint digest update")?;
            for (rowid, normalized) in &batch {
                update
                    .execute(params![rowid, fingerprint_digest(normalized)])
                    .context("store fingerprint digest")?;
            }
        }
        transaction
            .pragma_update(None, "user_version", HASHED_FINGERPRINT_VERSION)
            .context("save context data version")?;
        transaction
            .commit()
            .context("commit fingerprint digest upgrade")?;
        Ok(true)
    }

    fn upgrade_retention_columns(&self) -> Result<()> {
        let columns = [
            ("messages", "expired_at", "expired_at INTEGER"),
            ("responses", "expired_at", "expired_at INTEGER"),
            ("responses", "history_len", "history_len INTEGER"),
            ("responses", "history_tip", "history_tip TEXT"),
            (
                "messages",
                "body_codec",
                "body_codec INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "messages",
                "body_bytes",
                "body_bytes INTEGER NOT NULL DEFAULT 0",
            ),
            ("messages", "body_checksum", "body_checksum BLOB"),
            ("messages", "body_kind", "body_kind TEXT"),
            ("messages", "body_tool", "body_tool TEXT"),
            (
                "responses",
                "body_codec",
                "body_codec INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "responses",
                "body_bytes",
                "body_bytes INTEGER NOT NULL DEFAULT 0",
            ),
            ("responses", "body_checksum", "body_checksum BLOB"),
        ];
        let mut missing = Vec::new();
        for (table, column, definition) in columns {
            if !self.has_column(table, column)? {
                missing.push((table, column, definition));
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        // A concurrent CLI may have completed the same schema upgrade.
        for (table, column, definition) in missing {
            if self.has_column(table, column)? {
                continue;
            }
            transaction
                .execute(&format!("ALTER TABLE {table} ADD COLUMN {definition}"), [])
                .with_context(|| format!("add context {table}.{column}"))?;
        }
        transaction.commit().context("commit context body schema")?;
        Ok(())
    }

    fn backfill_latest_response_anchor(&self) -> Result<()> {
        let done: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM maintenance WHERE name = 'response-anchors')",
            [],
            |row| row.get(0),
        )?;
        if done {
            return Ok(());
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let done: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM maintenance WHERE name = 'response-anchors')",
            [],
            |row| row.get(0),
        )?;
        if done {
            return Ok(());
        }
        transaction.execute(
            "WITH latest AS (
                 SELECT rowid AS id, ROW_NUMBER() OVER (
                     PARTITION BY session_key ORDER BY created_at DESC, rowid DESC
                 ) AS rank FROM responses
             ) UPDATE responses SET
                 history_len = (SELECT COUNT(*) FROM history h WHERE h.session_key = responses.session_key),
                 history_tip = (SELECT h.reference FROM history h
                     WHERE h.session_key = responses.session_key ORDER BY h.position DESC LIMIT 1)
             WHERE history_len IS NULL AND rowid IN (SELECT id FROM latest WHERE rank = 1)
               AND revision = (SELECT s.revision FROM sessions s WHERE s.session_key = responses.session_key)
               AND created_at > (SELECT s.updated_at FROM sessions s WHERE s.session_key = responses.session_key)",
            [],
        ).context("backfill current response history anchor")?;
        transaction.execute(
            "INSERT INTO maintenance(name,at) VALUES ('response-anchors',?1)",
            [unix_now()],
        )?;
        transaction
            .commit()
            .context("commit context response anchors")?;
        Ok(())
    }

    /// Migrate bounded batches; session age never permits deleting history.
    pub(crate) fn maintenance_candidates(&self, limit: usize) -> Result<Vec<String>> {
        let mut statement = self.connection.prepare(
            "SELECT session_key FROM (
                SELECT session_key, MIN(created_at) AS oldest FROM messages
                WHERE (body_bytes = 0 AND item_json <> '' AND expired_at IS NULL)
                   OR length(fingerprint) > 64 GROUP BY session_key
                UNION ALL
                SELECT session_key, MIN(created_at) AS oldest FROM responses
                WHERE body_bytes = 0 AND output_json <> '' AND expired_at IS NULL GROUP BY session_key
             ) GROUP BY session_key ORDER BY MIN(oldest) LIMIT ?1",
        ).context("prepare lossless body migration")?;
        let rows = statement.query_map([limit as i64], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .context("read body migration candidates")
    }

    pub(crate) fn maintain_session(&mut self, key: &str) -> Result<()> {
        let transaction = self
            .connection
            .transaction()
            .context("begin lossless body migration")?;
        let mut processed = 0;
        for (table, column) in [("messages", "item_json"), ("responses", "output_json")] {
            let ids = {
                let mut statement = transaction.prepare(&format!(
                    "SELECT rowid FROM {table} WHERE session_key = ?1 AND body_bytes = 0
                     AND {column} <> '' AND expired_at IS NULL ORDER BY rowid LIMIT 128"
                ))?;
                statement
                    .query_map([key], |row| row.get::<_, i64>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            for id in ids {
                if processed >= BODY_BATCH_BYTES {
                    break;
                }
                let raw: String = transaction.query_row(
                    &format!("SELECT {column} FROM {table} WHERE rowid = ?1"),
                    [id],
                    |row| row.get(0),
                )?;
                let tags = if table == "messages" {
                    Some(
                        serde_json::from_str::<BodyTags>(&raw)
                            .context("read message tags for compression")?,
                    )
                } else {
                    None
                };
                processed += raw.len();
                let encoded = body::encode(raw, true)?;
                transaction.execute(
                    &format!(
                        "UPDATE {table} SET {column} = ?2, body_codec = ?3,
                    body_bytes = ?4, body_checksum = ?5 WHERE rowid = ?1"
                    ),
                    params![
                        id,
                        encoded.payload,
                        encoded.codec,
                        encoded.bytes,
                        encoded.checksum
                    ],
                )?;
                if let Some(tags) = tags {
                    transaction.execute(
                        "UPDATE messages SET body_kind = ?2, body_tool = ?3 WHERE rowid = ?1",
                        params![id, tags.role.or(tags.item_type), tags.name],
                    )?;
                }
            }
        }
        let fingerprints = {
            let mut statement = transaction.prepare(
                "SELECT reference, fingerprint FROM messages
                WHERE session_key = ?1 AND length(fingerprint) > 64 LIMIT 128",
            )?;
            statement
                .query_map([key], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (reference, fingerprint) in fingerprints {
            transaction.execute(
                "UPDATE messages SET fingerprint = ?3 WHERE session_key = ?1 AND reference = ?2",
                params![key, reference, fingerprint_digest(&fingerprint)],
            )?;
        }
        transaction
            .commit()
            .context("commit lossless body migration")?;
        Ok(())
    }

    pub(crate) fn storage_stats(&self) -> Result<Value> {
        let mut result = json!({});
        for (table, column) in [("messages", "item_json"), ("responses", "output_json")] {
            let (count, original, stored, compressed, missing): (i64,i64,i64,i64,i64) = self.connection.query_row(
                &format!("SELECT COUNT(*), COALESCE(SUM(CASE WHEN body_bytes > 0 THEN body_bytes
                    ELSE length(CAST({column} AS BLOB)) END),0), COALESCE(SUM(length(CAST({column} AS BLOB))),0),
                    COALESCE(SUM(body_codec = 1 AND {column} <> '' AND expired_at IS NULL),0),
                    COALESCE(SUM({column} = '' OR expired_at IS NOT NULL),0) FROM {table}"), [],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
            result[table] = json!({"count":count,"original_bytes":original,"stored_bytes":stored,
                "compressed":compressed,"missing":missing});
        }
        result["missing_messages"] = result["messages"]["missing"].clone();
        result["page_count"] = json!(self.connection.query_row(
            "PRAGMA page_count",
            [],
            |row| row.get::<_, i64>(0)
        )?);
        result["free_pages"] = json!(self.connection.query_row(
            "PRAGMA freelist_count",
            [],
            |row| row.get::<_, i64>(0)
        )?);
        let (busy, log, checkpointed) = self.checkpoint()?;
        result["wal"] = json!({"busy":busy,"log_pages":log,"checkpointed_pages":checkpointed});
        Ok(result)
    }

    pub(crate) fn compact(&self) -> Result<()> {
        self.connection
            .execute_batch("VACUUM")
            .context("compact context database")?;
        self.checkpoint()?;
        Ok(())
    }

    fn checkpoint(&self) -> Result<(i64, i64, i64)> {
        let checkpoint: (i64, i64, i64) =
            self.connection
                .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        if checkpoint.0 != 0 || checkpoint.1 != checkpoint.2 || checkpoint.1 < 2048 {
            tracing::debug!(
                busy = checkpoint.0,
                log_pages = checkpoint.1,
                checkpointed_pages = checkpoint.2,
                "context WAL checkpoint"
            );
            return Ok(checkpoint);
        }
        // A pinned reader must not stall forwarding just to reclaim WAL space.
        self.connection.busy_timeout(std::time::Duration::ZERO)?;
        let truncated = self
            .connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            });
        self.connection
            .busy_timeout(std::time::Duration::from_secs(5))?;
        truncated.context("truncate checkpointed context WAL")
    }

    pub(crate) fn reclaim_archive(&self) -> Result<bool> {
        // This pragma yields one row per page; stopping after the first row
        // would return only one page even when a larger batch was requested.
        {
            let mut statement = self.connection.prepare("PRAGMA incremental_vacuum(8192)")?;
            let mut pages = statement.query([])?;
            while pages
                .next()
                .context("reclaim context archive pages")?
                .is_some()
            {}
        }
        self.checkpoint()?;
        let free_pages: i64 = self
            .connection
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
        let auto_vacuum: i64 = self
            .connection
            .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))?;
        Ok(auto_vacuum == 2 && free_pages > 0)
    }

    /// Context databases written before response mapping existed lack the
    /// recorded output column, so an in-place upgrade has to add it.
    fn upgrade_response_output(&self) -> Result<()> {
        let mut statement = self
            .connection
            .prepare("PRAGMA table_info(responses)")
            .context("inspect context responses table")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .context("read context responses columns")?;
        let mut present = false;
        for column in columns {
            if column.context("read context responses column")? == "output_json" {
                present = true;
            }
        }
        drop(statement);
        if present {
            return Ok(());
        }
        self.connection
            .execute(
                "ALTER TABLE responses ADD COLUMN output_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )
            .context("add context responses output column")?;
        Ok(())
    }

    fn upgrade_reasoning_fingerprints(&self) -> Result<()> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .context("read context data version")?;
        if version >= REASONING_FINGERPRINT_VERSION {
            return Ok(());
        }
        let transaction = self
            .connection
            .unchecked_transaction()
            .context("begin reasoning fingerprint upgrade")?;
        // Old fingerprints treated omitted reasoning content and null as edits.
        transaction
            .execute(
                "UPDATE messages SET fingerprint = json_remove(fingerprint, '$.content')
                 WHERE json_type(fingerprint, '$.content') = 'null'
                   AND json_extract(fingerprint, '$.type') = 'reasoning'",
                [],
            )
            .context("normalize archived reasoning fingerprints")?;
        transaction
            .pragma_update(None, "user_version", REASONING_FINGERPRINT_VERSION)
            .context("save context data version")?;
        transaction
            .commit()
            .context("commit reasoning fingerprint upgrade")
    }

    /// Blocks written before compression sizes were tracked report zero, which
    /// the statistics show as "no saving recorded" rather than a wrong number.
    fn upgrade_block_tokens(&self) -> Result<()> {
        for (column, definition) in [
            (
                "covered_tokens",
                "covered_tokens INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "replacement_tokens",
                "replacement_tokens INTEGER NOT NULL DEFAULT 0",
            ),
        ] {
            if self.has_column("blocks", column)? {
                continue;
            }
            self.connection
                .execute(&format!("ALTER TABLE blocks ADD COLUMN {definition}"), [])
                .with_context(|| format!("add context block {column} column"))?;
        }
        Ok(())
    }

    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .with_context(|| format!("inspect context {table} table"))?;
        let mut rows = statement
            .query([])
            .with_context(|| format!("read context {table} columns"))?;
        while let Some(row) = rows.next().context("read context column")? {
            let name: String = row.get(1).context("read context column name")?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn nudge_state(&self, key: &str) -> Result<NudgeState> {
        let encoded: Option<String> = self
            .connection
            .query_row(
                "SELECT state_json FROM nudge_state WHERE session_key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .context("read context nudge state")?;
        match encoded {
            Some(encoded) => serde_json::from_str(&encoded).context("decode context nudge state"),
            None => Ok(NudgeState::default()),
        }
    }

    pub(super) fn save_nudge_state(&self, key: &str, state: &NudgeState) -> Result<()> {
        let encoded = serde_json::to_string(state).context("encode context nudge state")?;
        self.connection
            .execute(
                "INSERT INTO nudge_state (session_key, state_json) VALUES (?1, ?2)
             ON CONFLICT(session_key) DO UPDATE SET state_json = excluded.state_json",
                params![key, encoded],
            )
            .context("save context nudge state")?;
        Ok(())
    }

    pub fn ensure_session(&mut self, key: &str) -> Result<SessionRow> {
        self.connection
            .execute(
                "INSERT OR IGNORE INTO sessions (session_key, revision, next_ref, next_block, updated_at)
                 VALUES (?1, 0, 1, 1, ?2)",
                params![key, unix_now()],
            )
            .context("create context session")?;
        self.session_row(&self.connection, key)
    }

    fn session_row(&self, connection: &Connection, key: &str) -> Result<SessionRow> {
        connection
            .query_row(
                "SELECT revision, next_ref, next_block FROM sessions WHERE session_key = ?1",
                params![key],
                |row| {
                    Ok(SessionRow {
                        revision: row.get(0)?,
                        next_ref: row.get::<_, i64>(1)?.max(1) as u64,
                        next_block: row.get::<_, i64>(2)?.max(1) as u64,
                    })
                },
            )
            .optional()
            .context("read context session")?
            .context("context session disappeared")
    }

    /// Records the client-visible output of a completed response so a later
    /// chained request can rebuild its full input from the archive.
    pub fn record_response(
        &mut self,
        key: &str,
        response_id: &str,
        revision: i64,
        output: &[Value],
    ) -> Result<()> {
        let output_json =
            serde_json::to_string(output).context("encode recorded response output")?;
        let encoded = body::encode(output_json, false)?;
        let (history_len, history_tip) = self.history_anchor(key)?;
        self.connection
            .execute(
                "INSERT INTO responses (session_key, response_id, revision, output_json, created_at, history_len, history_tip,
                     body_codec, body_bytes, body_checksum)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(session_key, response_id) DO UPDATE SET
                   revision = excluded.revision,
                   output_json = excluded.output_json,
                   body_codec = excluded.body_codec,
                   body_bytes = excluded.body_bytes,
                   body_checksum = excluded.body_checksum,
                   expired_at = NULL,
                   history_len = excluded.history_len,
                   history_tip = excluded.history_tip",
                params![key, response_id, revision, encoded.payload, unix_now(), history_len as i64, history_tip, encoded.codec, encoded.bytes, encoded.checksum],
            )
            .context("record context response")?;
        Ok(())
    }

    pub fn response_output(&self, key: &str, response_id: &str) -> Result<Option<Vec<Value>>> {
        let recorded: Option<(StoredBody, Option<i64>)> = self
            .connection
            .query_row(
                "SELECT output_json, body_codec, body_bytes, body_checksum, expired_at FROM responses WHERE session_key = ?1 AND response_id = ?2",
                params![key, response_id],
                |row| Ok((stored_body(row, 0)?, row.get(4)?)),
            )
            .optional()
            .context("read recorded response output")?;
        recorded
            .map(|(json, expired_at)| {
                if expired_at.is_some() {
                    bail!("response {response_id} output expired from the context archive")
                }
                serde_json::from_str(&body::decode(json.0, json.1, json.2, json.3.as_deref())?)
                    .context("parse recorded response output")
            })
            .transpose()
    }

    pub(crate) fn response_anchor(
        &self,
        key: &str,
        response_id: &str,
    ) -> Result<Option<ResponseAnchor>> {
        let record: Option<StoredResponseAnchor> = self
            .connection
            .query_row(
                "SELECT output_json, body_codec, body_bytes, body_checksum, expired_at, history_len, history_tip FROM responses
                 WHERE session_key = ?1 AND response_id = ?2",
                params![key, response_id],
                |row| Ok((stored_body(row, 0)?, row.get(4)?, row.get(5)?, row.get(6)?)),
            )
            .optional()
            .context("read context response anchor")?;
        let Some((output, expired_at, history_len, history_tip)) = record else {
            return Ok(None);
        };
        if expired_at.is_some() {
            bail!("response {response_id} output expired from the context archive");
        }
        let output = serde_json::from_str(&body::decode(
            output.0,
            output.1,
            output.2,
            output.3.as_deref(),
        )?)
        .context("parse recorded response output")?;
        let history_len = history_len
            .map(|length| usize::try_from(length).context("response history anchor length"))
            .transpose()?;
        Ok(Some((output, history_len, history_tip)))
    }

    pub(crate) fn history_anchor(&self, key: &str) -> Result<(usize, Option<String>)> {
        let (length, tip): (i64, Option<String>) = self
            .connection
            .query_row(
                "SELECT COUNT(*), (SELECT h.reference FROM history h
                 WHERE h.session_key = ?1 ORDER BY h.position DESC LIMIT 1)
             FROM history WHERE session_key = ?1",
                [key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .context("read context history anchor")?;
        Ok((
            usize::try_from(length).context("context history anchor length")?,
            tip,
        ))
    }

    pub(crate) fn validate_history_anchor(
        &self,
        key: &str,
        length: usize,
        tip: Option<&str>,
    ) -> Result<()> {
        let (current_len, _) = self.history_anchor(key)?;
        if length > current_len {
            bail!("context history anchor is ahead of the archive");
        }
        if length > 0 && tip.is_none() {
            bail!("context history anchor is missing its tip reference");
        }
        if let Some(expected) = tip {
            if length == 0 {
                bail!("empty context history anchor has a fingerprint");
            }
            let actual: Option<String> = self.connection.query_row(
                "SELECT h.reference FROM history h WHERE h.session_key = ?1 AND h.position = ?2",
                params![key, length as i64 - 1],
                |row| row.get(0),
            ).optional().context("read context anchor fingerprint")?;
            if actual.as_deref() != Some(expected) {
                bail!("context history anchor does not match the archive");
            }
        }
        Ok(())
    }

    /// Continues from a known history prefix without loading expired bodies.
    /// An already appended prefix is deduplicated; divergent suffixes fork.
    pub(crate) fn sync_history_from(
        &mut self,
        key: &str,
        items: &[Value],
        fingerprint: impl Fn(&Value) -> Result<String>,
        prefix_len: usize,
        source: Option<&str>,
    ) -> Result<()> {
        let mut session = self.ensure_session(key)?;
        if let Some(source) = source {
            if source == key || session.next_ref != 1 || !self.history(key)?.is_empty() {
                bail!("context archive inheritance requires a new child session");
            }
            let parent = self.ensure_session(source)?;
            let transaction = self
                .connection
                .transaction()
                .context("begin inherited context archive")?;
            for (table, columns) in [
                (
                    "messages",
                    "reference,fingerprint,item_json,created_at,expired_at,body_codec,body_bytes,body_checksum,body_kind,body_tool",
                ),
                ("history", "position,reference"),
                (
                    "blocks",
                    "block_id,tier,topic,summary,direct_refs,full_refs,start_position,end_position,superseded_by,covered_tokens,replacement_tokens,created_at",
                ),
                ("segments", "revision,position,end_position,kind,reference"),
            ] {
                // Identifiers are constants; all request-derived values stay bound.
                transaction.execute(
                    &format!("INSERT INTO {table} (session_key,{columns}) SELECT ?1,{columns} FROM {table} WHERE session_key=?2"),
                    params![key, source],
                ).context("copy inherited context archive rows")?;
            }
            transaction.execute(
                "UPDATE sessions SET revision=?2,next_ref=?3,next_block=?4,updated_at=?5 WHERE session_key=?1",
                params![key, parent.revision, parent.next_ref, parent.next_block, unix_now()],
            ).context("initialize inherited context session")?;
            transaction
                .commit()
                .context("commit inherited context archive")?;
            session = parent;
        }
        let history = self.history(key)?;
        let history_len = history.len();
        if prefix_len > history_len {
            bail!("context history anchor is ahead of the archive");
        }
        if items.is_empty() {
            return Ok(());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT m.fingerprint FROM history h JOIN messages m
               ON m.session_key = h.session_key AND m.reference = h.reference
             WHERE h.session_key = ?1 AND h.position >= ?2 ORDER BY h.position",
            )
            .context("prepare context continuation suffix")?;
        let suffix = statement
            .query_map(params![key, prefix_len as i64], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("read context continuation suffix")?;
        drop(statement);
        let fingerprints = items.iter().map(&fingerprint).collect::<Result<Vec<_>>>()?;
        let common = suffix
            .iter()
            .zip(&fingerprints)
            .take_while(|(left, right)| left == right)
            .count();
        if common == items.len() {
            return Ok(());
        }
        let divergence = prefix_len + common;
        let kept: Vec<SegmentRow> = self
            .segments(key, session.revision)?
            .into_iter()
            .filter(|segment| segment.end_position < divergence as i64)
            .collect();
        let mut coverage = kept.iter().peekable();
        let uncovered: Vec<&HistoryEntry> = history
            .iter()
            .take(divergence)
            .filter(|entry| {
                while coverage
                    .peek()
                    .is_some_and(|segment| segment.end_position < entry.position)
                {
                    coverage.next();
                }
                coverage
                    .peek()
                    .is_none_or(|segment| segment.position > entry.position)
            })
            .collect();
        for entry in &uncovered {
            if self.message_json(key, &entry.reference)?.is_none() {
                bail!(
                    "cannot fork context history: original {} expired from the archive",
                    entry.reference
                );
            }
        }
        if divergence < history_len || !uncovered.is_empty() {
            let next_revision = session.revision + 1;
            let transaction = self
                .connection
                .transaction()
                .context("begin context continuation fork")?;
            transaction
                .execute("DELETE FROM segments WHERE session_key = ?1", [key])
                .context("clear segments for continuation fork")?;
            for segment in &kept {
                insert_segment(&transaction, key, next_revision, segment)?;
            }
            for entry in &uncovered {
                insert_segment(
                    &transaction,
                    key,
                    next_revision,
                    &SegmentRow {
                        position: entry.position,
                        end_position: entry.position,
                        kind: SegmentKind::Message,
                        reference: entry.reference.clone(),
                    },
                )?;
            }
            transaction
                .execute(
                    "DELETE FROM history WHERE session_key = ?1 AND position >= ?2",
                    params![key, divergence as i64],
                )
                .context("trim context history at continuation fork")?;
            transaction
                .execute(
                    "UPDATE sessions SET revision = ?2, updated_at = ?3 WHERE session_key = ?1",
                    params![key, next_revision, unix_now()],
                )
                .context("bump context revision for continuation fork")?;
            transaction
                .execute("DELETE FROM nudge_state WHERE session_key = ?1", [key])
                .context("reset context nudge at continuation fork")?;
            transaction
                .commit()
                .context("commit context continuation fork")?;
            session.revision = next_revision;
        }
        let mut next_ref = session.next_ref;
        let transaction = self
            .connection
            .transaction()
            .context("begin context continuation")?;
        for (offset, item) in items.iter().enumerate().skip(common) {
            let reference = format!("m{next_ref:06}");
            let position = prefix_len + offset;
            let encoded =
                serde_json::to_string(item).context("encode context continuation item")?;
            let stored = body::encode(encoded, false)?;
            transaction.execute(
                "INSERT INTO messages (session_key, reference, fingerprint, item_json, created_at,
                    body_codec, body_bytes, body_checksum, body_kind, body_tool)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![key, reference, fingerprints[offset], stored.payload, unix_now(), stored.codec,
                    stored.bytes, stored.checksum, item.get("role").or_else(|| item.get("type")).and_then(Value::as_str),
                    item.get("name").and_then(Value::as_str)],
            ).context("append context continuation message")?;
            transaction
                .execute(
                    "INSERT INTO history (session_key, position, reference) VALUES (?1, ?2, ?3)",
                    params![key, position as i64, reference],
                )
                .context("append context continuation history")?;
            insert_segment(
                &transaction,
                key,
                session.revision,
                &SegmentRow {
                    position: position as i64,
                    end_position: position as i64,
                    kind: SegmentKind::Message,
                    reference,
                },
            )?;
            next_ref += 1;
        }
        transaction
            .execute(
                "UPDATE sessions SET next_ref = ?2, updated_at = ?3 WHERE session_key = ?1",
                params![key, next_ref as i64, unix_now()],
            )
            .context("update context continuation reference counter")?;
        transaction
            .commit()
            .context("commit context continuation")?;
        Ok(())
    }

    pub fn response_owner(&self, response_id: &str) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT session_key FROM responses WHERE response_id = ?1",
                params![response_id],
                |row| row.get(0),
            )
            .optional()
            .context("read context response owner")
    }

    /// Returns the archived originals in conversation order.
    pub fn history_items(&self, key: &str) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        for entry in self.history(key)? {
            let encoded = self
                .message_json(key, &entry.reference)?
                .with_context(|| format!("context history is missing {}", entry.reference))?;
            items.push(serde_json::from_str(&encoded).context("parse archived history item")?);
        }
        Ok(items)
    }

    pub fn history(&self, key: &str) -> Result<Vec<HistoryEntry>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT h.position, h.reference
                 FROM history h JOIN messages m
                   ON m.session_key = h.session_key AND m.reference = h.reference
                 WHERE h.session_key = ?1 ORDER BY h.position",
            )
            .context("prepare history query")?;
        let rows = statement
            .query_map(params![key], |row| {
                Ok(HistoryEntry {
                    position: row.get(0)?,
                    reference: row.get(1)?,
                })
            })
            .context("query context history")?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context("read context history row")?);
        }
        Ok(entries)
    }

    /// Archived history length and the length of its common prefix with
    /// `incoming` fingerprints. Rows are compared one at a time, so the
    /// archived fingerprints are never held in memory together.
    pub fn history_prefix(
        &self,
        key: &str,
        mut incoming: impl Iterator<Item = Result<String>>,
    ) -> Result<(usize, usize)> {
        let history_len: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM history h JOIN messages m
                   ON m.session_key = h.session_key AND m.reference = h.reference
                 WHERE h.session_key = ?1",
                params![key],
                |row| row.get(0),
            )
            .context("count context history")?;
        let history_len = usize::try_from(history_len).context("context history length")?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT m.fingerprint
                 FROM history h JOIN messages m
                   ON m.session_key = h.session_key AND m.reference = h.reference
                 WHERE h.session_key = ?1 ORDER BY h.position",
            )
            .context("prepare history prefix query")?;
        let mut rows = statement
            .query(params![key])
            .context("query context history prefix")?;
        let mut divergence = 0;
        while let Some(row) = rows.next().context("read context history prefix row")? {
            let Some(next) = incoming.next() else { break };
            let archived = row
                .get_ref(0)
                .context("read context history fingerprint")?
                .as_str()
                .context("context history fingerprint is not text")?;
            if archived != next? {
                break;
            }
            divergence += 1;
        }
        Ok((history_len, divergence))
    }

    pub fn message_json(&self, key: &str, reference: &str) -> Result<Option<String>> {
        let record: Option<(StoredBody, Option<i64>)> = self
            .connection
            .query_row(
                "SELECT item_json, body_codec, body_bytes, body_checksum, expired_at FROM messages
             WHERE session_key = ?1 AND reference = ?2",
                params![key, reference],
                |row| Ok((stored_body(row, 0)?, row.get(4)?)),
            )
            .optional()
            .context("read archived message")?;
        let Some((stored, expired_at)) = record else {
            return Ok(None);
        };
        if expired_at.is_some() || matches!(&stored.0, SqlValue::Text(raw) if raw.is_empty()) {
            bail!("original {reference} expired from the context archive");
        }
        Ok(Some(body::decode(
            stored.0,
            stored.1,
            stored.2,
            stored.3.as_deref(),
        )?))
    }

    pub(crate) fn expired_message_count(&self, key: &str) -> Result<u64> {
        let count: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_key = ?1 AND expired_at IS NOT NULL",
                [key],
                |row| row.get(0),
            )
            .context("count expired context archive bodies")?;
        u64::try_from(count).context("expired context body count")
    }

    pub(crate) fn expired_direct_refs(&self, key: &str, refs: &[DirectRef]) -> Result<Vec<String>> {
        let mut expired = Vec::new();
        for reference in refs
            .iter()
            .filter(|reference| reference.kind == SegmentKind::Message)
        {
            let exists: Option<(i64, Option<i64>)> = self.connection.query_row(
                "SELECT length(item_json), expired_at FROM messages WHERE session_key = ?1 AND reference = ?2",
                params![key, reference.reference],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().context("check direct reference expiration")?;
            if exists.is_none_or(|(length, expired_at)| expired_at.is_some() || length == 0) {
                expired.push(reference.reference.clone());
            }
        }
        Ok(expired)
    }

    pub(super) fn session_preview(&self, key: &str) -> Result<Option<String>> {
        let mut statement = self.connection.prepare(
            "SELECT reference FROM messages WHERE session_key = ?1 AND expired_at IS NULL
             AND item_json <> '' AND (body_kind = 'user' OR body_kind IS NULL) ORDER BY reference",
        )?;
        let mut rows = statement.query([key])?;
        while let Some(row) = rows.next()? {
            let reference: String = row.get(0)?;
            let raw = self
                .message_json(key, &reference)?
                .context("session preview original missing")?;
            let item = serde_json::from_str(&raw).context("decode session preview message")?;
            if let Some(preview) = crate::clients::session_titles::request_preview(&item) {
                return Ok(Some(preview));
            }
        }
        Ok(None)
    }

    /// Aligns stored history with the client-sent items.
    ///
    /// Returns the session row after alignment and whether a new revision was
    /// opened. A mismatch, a shorter input or an edited item opens a revision
    /// and keeps the superseded originals archived.
    ///
    /// Only items after the divergence point are fingerprinted twice and
    /// encoded, so an unchanged prefix never exists as owned strings.
    pub fn sync_history(
        &mut self,
        key: &str,
        items: &[Value],
        fingerprint: impl Fn(&Value) -> Result<String>,
    ) -> Result<(SessionRow, bool)> {
        let mut session = self.ensure_session(key)?;
        let (_, divergence) = self.history_prefix(key, items.iter().map(&fingerprint))?;
        let history = self.history(key)?;
        let kept: Vec<SegmentRow> = self
            .segments(key, session.revision)?
            .into_iter()
            .filter(|segment| segment.end_position < divergence as i64)
            .collect();
        let mut coverage = kept.iter().peekable();
        // A fork inside a block invalidates its summary, not its unchanged
        // prefix. This also recovers holes left by older gateway versions.
        let uncovered: Vec<&HistoryEntry> = history
            .iter()
            .take(divergence)
            .filter(|entry| {
                while coverage
                    .peek()
                    .is_some_and(|segment| segment.end_position < entry.position)
                {
                    coverage.next();
                }
                coverage
                    .peek()
                    .is_none_or(|segment| segment.position > entry.position)
            })
            .collect();
        let changed = divergence < history.len() || !uncovered.is_empty();
        let transaction = self
            .connection
            .transaction()
            .context("begin history synchronization")?;
        // A fork through a block exposes part of its raw prefix. Restore only
        // visible originals from the fingerprint-verified full replay; leave
        // bodies still covered by summaries expired. This also repairs views
        // left incomplete by older versions, even without a new divergence.
        for (position, reference) in kept
            .iter()
            .filter(|segment| segment.kind == SegmentKind::Message)
            .map(|segment| (segment.position, segment.reference.as_str()))
            .chain(
                uncovered
                    .iter()
                    .map(|entry| (entry.position, entry.reference.as_str())),
            )
        {
            let expired: bool = transaction
                .query_row(
                    "SELECT expired_at IS NOT NULL OR item_json = '' FROM messages
                     WHERE session_key = ?1 AND reference = ?2",
                    params![key, reference],
                    |row| row.get(0),
                )
                .context("check replayed visible original")?;
            if !expired {
                continue;
            }
            let item = &items[position as usize];
            let stored = body::encode(serde_json::to_string(item)?, false)?;
            transaction
                .execute(
                    "UPDATE messages SET item_json = ?3, expired_at = NULL, body_codec = ?4,
                        body_bytes = ?5, body_checksum = ?6, body_kind = ?7, body_tool = ?8
                 WHERE session_key = ?1 AND reference = ?2
                   AND (expired_at IS NOT NULL OR item_json = '')",
                    params![
                        key,
                        reference,
                        stored.payload,
                        stored.codec,
                        stored.bytes,
                        stored.checksum,
                        item.get("role")
                            .or_else(|| item.get("type"))
                            .and_then(Value::as_str),
                        item.get("name").and_then(Value::as_str)
                    ],
                )
                .context("restore replayed visible original")?;
        }
        if changed {
            let next_revision = session.revision + 1;
            transaction
                .execute("DELETE FROM segments WHERE session_key = ?1", params![key])
                .context("clear segments for fork")?;
            for segment in &kept {
                insert_segment(&transaction, key, next_revision, segment)?;
            }
            for entry in &uncovered {
                insert_segment(
                    &transaction,
                    key,
                    next_revision,
                    &SegmentRow {
                        position: entry.position,
                        end_position: entry.position,
                        kind: SegmentKind::Message,
                        reference: entry.reference.clone(),
                    },
                )?;
            }
            transaction
                .execute(
                    "DELETE FROM history WHERE session_key = ?1 AND position >= ?2",
                    params![key, divergence as i64],
                )
                .context("trim history at fork")?;
            transaction
                .execute(
                    "UPDATE sessions SET revision = ?2, updated_at = ?3 WHERE session_key = ?1",
                    params![key, next_revision, unix_now()],
                )
                .context("bump session revision")?;
            transaction
                .execute("DELETE FROM nudge_state WHERE session_key = ?1", [key])
                .context("reset context nudge at history fork")?;
            session.revision = next_revision;
        }
        let mut next_ref = session.next_ref;
        for (index, item) in items.iter().enumerate().skip(divergence) {
            let reference = format!("m{next_ref:06}");
            let encoded = serde_json::to_string(item).context("encode history item")?;
            let stored = body::encode(encoded, false)?;
            transaction
                .execute(
                    "INSERT INTO messages (session_key, reference, fingerprint, item_json, created_at,
                        body_codec, body_bytes, body_checksum, body_kind, body_tool)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![key, reference, fingerprint(item)?, stored.payload, unix_now(), stored.codec,
                        stored.bytes, stored.checksum, item.get("role").or_else(|| item.get("type")).and_then(Value::as_str),
                        item.get("name").and_then(Value::as_str)],
                )
                .context("append archived message")?;
            transaction
                .execute(
                    "INSERT INTO history (session_key, position, reference) VALUES (?1, ?2, ?3)",
                    params![key, index as i64, reference],
                )
                .context("append history entry")?;
            insert_segment(
                &transaction,
                key,
                session.revision,
                &SegmentRow {
                    position: index as i64,
                    end_position: index as i64,
                    kind: SegmentKind::Message,
                    reference: reference.clone(),
                },
            )?;
            next_ref += 1;
        }
        transaction
            .execute(
                "UPDATE sessions SET next_ref = ?2, updated_at = ?3 WHERE session_key = ?1",
                params![key, next_ref as i64, unix_now()],
            )
            .context("update session reference counter")?;
        transaction
            .commit()
            .context("commit history synchronization")?;
        if !uncovered.is_empty() {
            tracing::info!(
                session = key,
                restored_messages = uncovered.len(),
                "restored uncovered archived messages to the context view"
            );
        }
        session.next_ref = next_ref;
        Ok((session, changed))
    }

    pub fn segments(&self, key: &str, revision: i64) -> Result<Vec<SegmentRow>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT position, end_position, kind, reference FROM segments
                 WHERE session_key = ?1 AND revision = ?2 ORDER BY position",
            )
            .context("prepare segment query")?;
        let rows = statement
            .query_map(params![key, revision], |row| {
                let kind: String = row.get(2)?;
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    kind,
                    row.get::<_, String>(3)?,
                ))
            })
            .context("query segments")?;
        let mut segments = Vec::new();
        for row in rows {
            let (position, end_position, kind, reference) = row.context("read segment row")?;
            let kind = SegmentKind::parse(&kind)
                .with_context(|| format!("unknown stored segment kind {kind}"))?;
            segments.push(SegmentRow {
                position,
                end_position,
                kind,
                reference,
            });
        }
        Ok(segments)
    }
    pub fn block(&self, key: &str, block_id: &str) -> Result<Option<BlockRow>> {
        self.connection
            .query_row(
                "SELECT block_id, tier, topic, summary, direct_refs, full_refs
                 FROM blocks WHERE session_key = ?1 AND block_id = ?2",
                params![key, block_id],
                |row| {
                    let tier_value: u8 = row.get(1)?;
                    let direct: String = row.get(4)?;
                    let full: String = row.get(5)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        tier_value,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        direct,
                        full,
                    ))
                },
            )
            .optional()
            .context("read context block")?
            .map(|row| {
                let (id, tier_value, topic, summary, direct, full) = row;
                let tier = Tier::from_u8(tier_value)
                    .with_context(|| format!("unknown stored tier {tier_value}"))?;
                let direct_refs: Vec<DirectRef> =
                    serde_json::from_str(&direct).context("decode block direct references")?;
                let full_refs: Vec<String> =
                    serde_json::from_str(&full).context("decode block full references")?;
                Ok(BlockRow {
                    id,
                    tier,
                    topic,
                    summary,
                    direct_refs,
                    full_refs,
                })
            })
            .transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_compression(
        &mut self,
        key: &str,
        expected_revision: i64,
        covered: &[SegmentRow],
        tier: Tier,
        topic: &str,
        summary: &str,
        direct_refs: &[DirectRef],
        full_refs: &[String],
        covered_tokens: u64,
        replacement_tokens: u64,
    ) -> Result<(String, i64)> {
        if covered.is_empty() {
            bail!("compression cover is empty");
        }
        for pair in covered.windows(2) {
            if pair[0].end_position + 1 != pair[1].position {
                bail!("compression cover is not contiguous");
            }
        }
        let current = self.segments(key, expected_revision)?;
        for segment in covered {
            let present = current.iter().any(|candidate| {
                candidate.position == segment.position
                    && candidate.end_position == segment.end_position
                    && candidate.reference == segment.reference
            });
            if !present {
                bail!("compression cover does not match the current view");
            }
        }
        let start = covered[0].position;
        let end = covered
            .last()
            .map(|segment| segment.end_position)
            .context("compression cover has no end")?;
        let session = self.ensure_session(key)?;
        if session.revision != expected_revision {
            bail!("context revision changed while compressing");
        }
        let block_id = format!("b{}", session.next_block);
        let new_revision = expected_revision + 1;
        let transaction = self.connection.transaction().context("begin compression")?;
        transaction
            .execute(
                "INSERT INTO blocks (session_key, block_id, tier, topic, summary, direct_refs,
                                     full_refs, start_position, end_position, superseded_by,
                                     covered_tokens, replacement_tokens, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, ?12)",
                params![
                    key,
                    block_id,
                    tier.as_u8(),
                    topic,
                    summary,
                    serde_json::to_string(direct_refs).context("encode block direct references")?,
                    serde_json::to_string(full_refs).context("encode block full references")?,
                    start,
                    end,
                    covered_tokens as i64,
                    replacement_tokens as i64,
                    unix_now()
                ],
            )
            .context("insert context block")?;
        for reference in direct_refs {
            if reference.kind == SegmentKind::Block {
                transaction
                    .execute(
                        "UPDATE blocks SET superseded_by = ?3
                         WHERE session_key = ?1 AND block_id = ?2",
                        params![key, reference.reference, block_id],
                    )
                    .context("mark superseded block")?;
            }
        }
        for segment in current
            .iter()
            .filter(|segment| segment.position < start || segment.end_position > end)
        {
            insert_segment(&transaction, key, new_revision, segment)?;
        }
        insert_segment(
            &transaction,
            key,
            new_revision,
            &SegmentRow {
                position: start,
                end_position: end,
                kind: SegmentKind::Block,
                reference: block_id.clone(),
            },
        )?;
        transaction
            .execute(
                "DELETE FROM segments WHERE session_key = ?1 AND revision = ?2",
                params![key, expected_revision],
            )
            .context("retire old segments")?;
        transaction
            .execute(
                "UPDATE sessions SET revision = ?2, next_block = next_block + 1, updated_at = ?3
                 WHERE session_key = ?1",
                params![key, new_revision, unix_now()],
            )
            .context("advance session revision")?;
        transaction
            .execute("DELETE FROM nudge_state WHERE session_key = ?1", [key])
            .context("reset context nudge after compression")?;
        transaction.commit().context("commit compression")?;
        Ok((block_id, new_revision))
    }

    pub fn restore_block(
        &mut self,
        key: &str,
        expected_revision: i64,
        block_id: &str,
    ) -> Result<i64> {
        let block = self
            .block(key, block_id)?
            .with_context(|| format!("context block {block_id} does not exist"))?;
        let current = self.segments(key, expected_revision)?;
        let target = current
            .iter()
            .find(|segment| segment.kind == SegmentKind::Block && segment.reference == block_id)
            .with_context(|| format!("context block {block_id} is not in the current view"))?
            .clone();
        let expired = self.expired_direct_refs(key, &block.direct_refs)?;
        if !expired.is_empty() {
            bail!(
                "cannot restore context block {block_id}: original messages expired ({})",
                expired.join(", ")
            );
        }
        let new_revision = expected_revision + 1;
        let transaction = self.connection.transaction().context("begin restore")?;
        for segment in current
            .iter()
            .filter(|segment| segment.position != target.position)
        {
            insert_segment(&transaction, key, new_revision, segment)?;
        }
        for reference in &block.direct_refs {
            insert_segment(
                &transaction,
                key,
                new_revision,
                &SegmentRow {
                    position: reference.position,
                    end_position: reference.end_position,
                    kind: reference.kind,
                    reference: reference.reference.clone(),
                },
            )?;
        }
        transaction
            .execute(
                "DELETE FROM segments WHERE session_key = ?1 AND revision = ?2",
                params![key, expected_revision],
            )
            .context("retire old segments on restore")?;
        transaction
            .execute(
                "UPDATE sessions SET revision = ?2, updated_at = ?3 WHERE session_key = ?1",
                params![key, new_revision, unix_now()],
            )
            .context("advance session revision on restore")?;
        transaction.commit().context("commit restore")?;
        Ok(new_revision)
    }

    pub fn search(&self, key: &str, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let pattern = format!("%{}%", query.replace('%', ""));
        let mut hits = Vec::new();
        let mut statement = self
            .connection
            .prepare(
                "SELECT block_id, tier, topic, summary FROM blocks
                 WHERE session_key = ?1 AND (topic LIKE ?2 OR summary LIKE ?2)
                 ORDER BY created_at DESC LIMIT ?3",
            )
            .context("prepare block search")?;
        let rows = statement
            .query_map(params![key, pattern, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u8>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .context("search blocks")?;
        for row in rows {
            let (reference, tier, topic, summary) = row.context("read block search hit")?;
            hits.push(SearchHit {
                kind: format!("t{tier}_block"),
                reference,
                title: topic,
                snippet: snippet(&summary),
            });
        }
        let mut statement = self.connection.prepare(
            "SELECT reference FROM messages WHERE session_key = ?1 AND expired_at IS NULL
             AND item_json <> '' ORDER BY created_at DESC, rowid DESC",
        )?;
        let mut rows = statement.query([key])?;
        let needle = query.as_bytes();
        while hits.len() < limit {
            let Some(row) = rows.next()? else {
                break;
            };
            let reference: String = row.get(0)?;
            let item = self
                .message_json(key, &reference)?
                .context("search original missing")?;
            let item_bytes = item.as_bytes();
            let matches = needle.is_empty()
                || (needle.len() <= item_bytes.len()
                    && item_bytes
                        .windows(needle.len())
                        .any(|window| window.eq_ignore_ascii_case(needle)));
            if matches {
                hits.push(SearchHit {
                    kind: "message".to_owned(),
                    reference,
                    title: String::new(),
                    snippet: snippet(&item),
                });
            }
        }
        hits.truncate(limit);
        Ok(hits)
    }

    pub fn tier_counts(
        &self,
        key: &str,
        revision: i64,
    ) -> Result<(super::types::TierCounts, u64, u64)> {
        let mut counts = super::types::TierCounts::default();
        for segment in self.segments(key, revision)? {
            match segment.kind {
                SegmentKind::Message => counts.messages += 1,
                SegmentKind::Block => {
                    if let Some(block) = self.block(key, &segment.reference)? {
                        match block.tier {
                            Tier::One => counts.t1_blocks += 1,
                            Tier::Two => counts.t2_blocks += 1,
                            Tier::Three => counts.t3_blocks += 1,
                        }
                    }
                }
            }
        }
        let (archive_messages, archive_bytes) = self
            .connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(CASE WHEN body_bytes > 0 AND expired_at IS NULL AND item_json <> ''
                    THEN body_bytes ELSE length(CAST(item_json AS BLOB)) END), 0) FROM messages WHERE session_key = ?1",
                params![key],
                |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
            )
            .context("count archived messages")?;
        Ok((counts, archive_messages, archive_bytes))
    }

    pub fn record_turn(&mut self, key: &str, sample: TurnSample) -> Result<()> {
        self.ensure_session(key)?;
        self.connection
            .execute(
                "INSERT INTO turns (session_key, at_ms, input_tokens, cached_tokens, output_tokens, cache_metrics_available, codex_thread_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    key,
                    sample.at_ms,
                    sample.input_tokens as i64,
                    sample.cached_tokens as i64,
                    sample.output_tokens as i64,
                    sample.cache_metrics_available,
                    sample.codex_thread_id
                ],
            )
            .context("record context turn usage")?;
        Ok(())
    }

    /// Per-session compression and cache aggregates for the console page. View
    /// sizes stay out of this query because only the engine can render a view.
    pub fn session_usage(&self) -> Result<Vec<SessionUsage>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT s.session_key, s.revision, s.updated_at,
                        (SELECT COUNT(*) FROM history h WHERE h.session_key = s.session_key),
                        (SELECT COUNT(*) FROM blocks b WHERE b.session_key = s.session_key),
                        (SELECT COALESCE(SUM(b.covered_tokens - b.replacement_tokens), 0)
                         FROM blocks b
                         WHERE b.session_key = s.session_key AND b.superseded_by IS NULL),
                        (SELECT COUNT(*) FROM turns t WHERE t.session_key = s.session_key),
                        (SELECT COALESCE(SUM(t.input_tokens), 0) FROM turns t
                         WHERE t.session_key = s.session_key),
                        (SELECT COALESCE(SUM(t.cached_tokens), 0) FROM turns t
                         WHERE t.session_key = s.session_key),
                        (SELECT COALESCE(SUM(t.output_tokens), 0) FROM turns t
                         WHERE t.session_key = s.session_key),
                        (SELECT COALESCE(SUM(t.input_tokens), 0) FROM turns t
                         WHERE t.session_key = s.session_key AND t.cache_metrics_available = 1),
                        (SELECT COALESCE(SUM(t.cached_tokens), 0) FROM turns t
                         WHERE t.session_key = s.session_key AND t.cache_metrics_available = 1),
                        (SELECT t.codex_thread_id FROM turns t WHERE t.session_key = s.session_key
                         AND t.codex_thread_id IS NOT NULL LIMIT 1)
                 FROM sessions s ORDER BY s.updated_at DESC",
            )
            .context("prepare session usage")?;
        let rows = statement
            .query_map([], |row| {
                Ok(SessionUsage {
                    session: row.get(0)?,
                    codex_thread_id: row.get(12)?,
                    revision: row.get(1)?,
                    updated_at: row.get(2)?,
                    messages: row.get::<_, i64>(3)?.max(0) as u64,
                    compressions: row.get::<_, i64>(4)?.max(0) as u64,
                    saved_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                    cache: CacheStats {
                        turns: row.get::<_, i64>(6)?.max(0) as u64,
                        input_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                        cached_tokens: row.get::<_, i64>(8)?.max(0) as u64,
                        output_tokens: row.get::<_, i64>(9)?.max(0) as u64,
                        hit_percent: None,
                        observed_input_tokens: row.get::<_, i64>(10)?.max(0) as u64,
                        observed_cached_tokens: row.get::<_, i64>(11)?.max(0) as u64,
                    },
                })
            })
            .context("read session usage")?;
        let mut usage = Vec::new();
        for row in rows {
            let mut row = row.context("read session usage row")?;
            row.cache.hit_percent = cache_hit_percent(
                row.cache.observed_cached_tokens,
                row.cache.observed_input_tokens,
            );
            usage.push(row);
        }
        Ok(usage)
    }

    pub fn session_summaries(&self) -> Result<Vec<SessionSummary>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT s.session_key, s.revision, s.updated_at,
                        (SELECT COUNT(*) FROM history h WHERE h.session_key = s.session_key),
                        (SELECT COUNT(*) FROM blocks b WHERE b.session_key = s.session_key)
                 FROM sessions s ORDER BY s.updated_at DESC",
            )
            .context("prepare session list")?;
        let rows = statement
            .query_map([], |row| {
                Ok(SessionSummary {
                    session: row.get(0)?,
                    revision: row.get(1)?,
                    updated_at: row.get(2)?,
                    messages: row.get::<_, i64>(3)? as u64,
                    blocks: row.get::<_, i64>(4)? as u64,
                })
            })
            .context("list context sessions")?;
        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row.context("read session summary")?);
        }
        Ok(summaries)
    }

    pub fn export(&self, key: &str, compact: bool) -> Result<Value> {
        let session = self
            .connection
            .query_row(
                "SELECT revision, next_ref, next_block FROM sessions WHERE session_key = ?1",
                params![key],
                |row| {
                    Ok(SessionRow {
                        revision: row.get(0)?,
                        next_ref: row.get::<_, i64>(1)?.max(1) as u64,
                        next_block: row.get::<_, i64>(2)?.max(1) as u64,
                    })
                },
            )
            .optional()
            .context("read session for export")?;
        let Some(session) = session else {
            bail!("context session does not exist");
        };
        let mut items = Vec::new();
        if compact {
            // Project metadata in one query: no N+1 body reads or JSON/body
            // copies in Rust. SQLite still scans the stored JSON for its tags.
            let mut statement = self
                .connection
                .prepare(
                    "SELECT h.position, h.reference, CASE WHEN m.body_bytes > 0 AND m.item_json <> ''
                            AND m.expired_at IS NULL THEN m.body_bytes ELSE length(CAST(m.item_json AS BLOB)) END,
                        CASE WHEN m.expired_at IS NOT NULL OR m.item_json = '' THEN 'expired'
                             ELSE COALESCE(m.body_kind, json_extract(CASE WHEN m.body_codec = 0 THEN NULLIF(m.item_json, '') END, '$.role'),
                                           json_extract(CASE WHEN m.body_codec = 0 THEN NULLIF(m.item_json, '') END, '$.type'), 'other') END,
                        COALESCE(m.body_tool, json_extract(CASE WHEN m.body_codec = 0 THEN NULLIF(m.item_json, '') END, '$.name')),
                        m.expired_at IS NOT NULL OR m.item_json = '',
                        CASE WHEN m.expired_at IS NOT NULL OR m.item_json = '' THEN 'missing'
                             WHEN m.body_codec = 1 THEN 'zstd' ELSE 'plain' END
                 FROM history h JOIN messages m
                   ON m.session_key = h.session_key AND m.reference = h.reference
                 WHERE h.session_key = ?1 ORDER BY h.position",
                )
                .context("prepare compact history export")?;
            let rows = statement
                .query_map(params![key], |row| {
                    Ok(json!({
                        "position": row.get::<_, i64>(0)?,
                        "reference": row.get::<_, String>(1)?,
                        "bytes": row.get::<_, i64>(2)?,
                        "kind": row.get::<_, String>(3)?,
                        "tool": row.get::<_, Option<String>>(4)?,
                        "expired": row.get::<_, bool>(5)?,
                        "storage": row.get::<_, String>(6)?,
                    }))
                })
                .context("export compact history")?;
            for row in rows {
                items.push(row.context("read compact history metadata")?);
            }
        } else {
            for entry in self.history(key)? {
                items.push(json!({
                    "position": entry.position,
                    "reference": entry.reference,
                    "item": self.message_json(key, &entry.reference)?.map(|raw| serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null)),
                }));
            }
        }
        let segments: Vec<Value> = self
            .segments(key, session.revision)?
            .into_iter()
            .map(|segment| {
                json!({
                    "position": segment.position,
                    "end_position": segment.end_position,
                    "kind": segment.kind.as_str(),
                    "reference": segment.reference,
                })
            })
            .collect();
        let mut statement = self
            .connection
            .prepare(
                "SELECT block_id, tier, topic, summary, direct_refs, full_refs,
                        start_position, end_position, superseded_by, created_at
                 FROM blocks WHERE session_key = ?1 ORDER BY created_at",
            )
            .context("prepare block export")?;
        let rows = statement
            .query_map(params![key], |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "tier": row.get::<_, u8>(1)?,
                    "topic": row.get::<_, String>(2)?,
                    "summary": row.get::<_, String>(3)?,
                    "direct_refs": row.get::<_, String>(4)?,
                    "full_refs": row.get::<_, String>(5)?,
                    "start_position": row.get::<_, i64>(6)?,
                    "end_position": row.get::<_, i64>(7)?,
                    "superseded_by": row.get::<_, Option<String>>(8)?,
                    "created_at": row.get::<_, i64>(9)?,
                }))
            })
            .context("export blocks")?;
        let mut blocks = Vec::new();
        for row in rows {
            blocks.push(row.context("read exported block")?);
        }
        let mut document = json!({
            "session": key,
            "revision": session.revision,
            "history": items,
            "segments": segments,
            "blocks": blocks,
        });
        if compact {
            document["compact"] = json!(true);
        }
        Ok(document)
    }
}

fn stored_body(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<StoredBody> {
    Ok((
        row.get(offset)?,
        row.get(offset + 1)?,
        row.get(offset + 2)?,
        row.get(offset + 3)?,
    ))
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.connection.busy_timeout(std::time::Duration::ZERO);
        let result = self
            .connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                row.get::<_, i64>(0)
            });
        if !matches!(result, Ok(0)) {
            tracing::debug!(
                ?result,
                "context WAL retained by another connection at close"
            );
        }
    }
}

fn insert_segment(
    transaction: &Transaction<'_>,
    key: &str,
    revision: i64,
    segment: &SegmentRow,
) -> Result<()> {
    transaction
        .execute(
            "INSERT INTO segments (session_key, revision, position, end_position, kind, reference)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                key,
                revision,
                segment.position,
                segment.end_position,
                segment.kind.as_str(),
                segment.reference
            ],
        )
        .context("insert context segment")?;
    Ok(())
}

fn snippet(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(240).collect()
}
