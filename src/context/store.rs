use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};

use super::types::{CacheStats, DirectRef, SegmentKind, SessionSummary, Tier};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    session_key TEXT PRIMARY KEY,
    revision INTEGER NOT NULL DEFAULT 0,
    next_ref INTEGER NOT NULL DEFAULT 1,
    next_block INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL,
    last_error TEXT
);
CREATE TABLE IF NOT EXISTS messages (
    session_key TEXT NOT NULL,
    reference TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    item_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
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
    created_at INTEGER NOT NULL,
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
    output_tokens INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS turns_session ON turns (session_key, at_ms);
";

/// Cached prompt tokens are a subset of the billed input tokens.
pub(crate) fn cache_hit_percent(cached_tokens: u64, input_tokens: u64) -> f64 {
    if input_tokens == 0 {
        return 0.0;
    }
    cached_tokens as f64 / input_tokens as f64 * 100.0
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
    pub fingerprint: String,
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
#[derive(Clone, Copy, Debug)]
pub(crate) struct TurnSample {
    pub at_ms: i64,
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionUsage {
    pub session: String,
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

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory().context("open in-memory context database")?;
        let store = Self { connection };
        store.configure()?;
        Ok(store)
    }

    fn configure(&self) -> Result<()> {
        self.connection
            .pragma_update(None, "journal_mode", "WAL")
            .context("set context journal mode")?;
        self.connection
            .pragma_update(None, "synchronous", "NORMAL")
            .context("set context synchronous mode")?;
        self.connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .context("set context busy timeout")?;
        self.connection
            .execute_batch(SCHEMA)
            .context("create context schema")?;
        self.upgrade_response_output()
            .context("upgrade context schema")?;
        self.upgrade_block_tokens()
            .context("upgrade context schema")?;
        // No lease survives a process restart, so stale rows must not block a
        // new turn after a crash.
        self.connection
            .execute("DELETE FROM leases", [])
            .context("clear stale context leases")?;
        Ok(())
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

    /// Takes the per-session execution lease for one turn.
    ///
    /// A different live holder makes the turn fail explicitly instead of
    /// interleaving two views of the same session.
    pub fn acquire_lease(&mut self, key: &str, owner: &str, ttl_seconds: i64) -> Result<()> {
        let now = unix_now();
        let expires_at = now.saturating_add(ttl_seconds);
        let transaction = self
            .connection
            .transaction()
            .context("begin context lease")?;
        let holder: Option<String> = transaction
            .query_row(
                "SELECT owner FROM leases WHERE session_key = ?1 AND expires_at > ?2",
                params![key, now],
                |row| row.get(0),
            )
            .optional()
            .context("read context lease")?;
        if holder.is_some_and(|holder| holder != owner) {
            bail!("context session is busy with another turn");
        }
        transaction
            .execute(
                "INSERT INTO leases (session_key, owner, acquired_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(session_key) DO UPDATE SET
                   owner = excluded.owner,
                   acquired_at = excluded.acquired_at,
                   expires_at = excluded.expires_at",
                params![key, owner, now, expires_at],
            )
            .context("write context lease")?;
        transaction.commit().context("commit context lease")?;
        Ok(())
    }

    pub fn release_lease(&mut self, key: &str, owner: &str) -> Result<()> {
        self.connection
            .execute(
                "DELETE FROM leases WHERE session_key = ?1 AND owner = ?2",
                params![key, owner],
            )
            .context("release context lease")?;
        Ok(())
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
        self.connection
            .execute(
                "INSERT INTO responses (session_key, response_id, revision, output_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_key, response_id) DO UPDATE SET
                   revision = excluded.revision,
                   output_json = excluded.output_json",
                params![key, response_id, revision, output_json, unix_now()],
            )
            .context("record context response")?;
        Ok(())
    }

    pub fn response_output(&self, key: &str, response_id: &str) -> Result<Option<Vec<Value>>> {
        let recorded: Option<String> = self
            .connection
            .query_row(
                "SELECT output_json FROM responses WHERE session_key = ?1 AND response_id = ?2",
                params![key, response_id],
                |row| row.get(0),
            )
            .optional()
            .context("read recorded response output")?;
        recorded
            .map(|json| serde_json::from_str(&json).context("parse recorded response output"))
            .transpose()
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
                "SELECT h.position, h.reference, m.fingerprint
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
                    fingerprint: row.get(2)?,
                })
            })
            .context("query context history")?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context("read context history row")?);
        }
        Ok(entries)
    }

    pub fn message_json(&self, key: &str, reference: &str) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT item_json FROM messages WHERE session_key = ?1 AND reference = ?2",
                params![key, reference],
                |row| row.get(0),
            )
            .optional()
            .context("read archived message")
    }

    /// Aligns stored history with the client-sent items.
    ///
    /// Returns the session row after alignment and whether a new revision was
    /// opened. A mismatch, a shorter input or an edited item opens a revision
    /// and keeps the superseded originals archived.
    pub fn sync_history(
        &mut self,
        key: &str,
        fingerprints: &[String],
        items: &[String],
    ) -> Result<(SessionRow, bool)> {
        if fingerprints.len() != items.len() {
            bail!("history fingerprint and item counts differ");
        }
        let mut session = self.ensure_session(key)?;
        let history = self.history(key)?;
        let divergence = history
            .iter()
            .zip(fingerprints.iter())
            .position(|(entry, fingerprint)| entry.fingerprint != *fingerprint)
            .unwrap_or_else(|| history.len().min(fingerprints.len()));
        let mut forked = false;
        if divergence < history.len() || fingerprints.len() < history.len() {
            forked = true;
            let next_revision = session.revision + 1;
            let keep_until = divergence as i64;
            let kept: Vec<SegmentRow> = self
                .segments(key, session.revision)?
                .into_iter()
                .filter(|segment| segment.end_position < keep_until)
                .collect();
            let transaction = self
                .connection
                .transaction()
                .context("begin fork transaction")?;
            transaction
                .execute("DELETE FROM segments WHERE session_key = ?1", params![key])
                .context("clear segments for fork")?;
            for segment in &kept {
                insert_segment(&transaction, key, next_revision, segment)?;
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
            transaction.commit().context("commit fork")?;
            session.revision = next_revision;
        }
        let mut next_ref = session.next_ref;
        let transaction = self
            .connection
            .transaction()
            .context("begin append transaction")?;
        for index in divergence..fingerprints.len() {
            let reference = format!("m{next_ref:06}");
            transaction
                .execute(
                    "INSERT INTO messages (session_key, reference, fingerprint, item_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![key, reference, fingerprints[index], items[index], unix_now()],
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
        transaction.commit().context("commit history append")?;
        session.next_ref = next_ref;
        Ok((session, forked))
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
        let mut statement = self
            .connection
            .prepare(
                "SELECT reference, item_json FROM messages
                 WHERE session_key = ?1 AND item_json LIKE ?2
                 ORDER BY created_at DESC LIMIT ?3",
            )
            .context("prepare message search")?;
        let rows = statement
            .query_map(params![key, pattern, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .context("search messages")?;
        for row in rows {
            let (reference, item) = row.context("read message search hit")?;
            hits.push(SearchHit {
                kind: "message".to_owned(),
                reference,
                title: String::new(),
                snippet: snippet(&item),
            });
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
                "SELECT COUNT(*), COALESCE(SUM(LENGTH(item_json)), 0) FROM messages WHERE session_key = ?1",
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
                "INSERT INTO turns (session_key, at_ms, input_tokens, cached_tokens, output_tokens)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    key,
                    sample.at_ms,
                    sample.input_tokens as i64,
                    sample.cached_tokens as i64,
                    sample.output_tokens as i64
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
                         WHERE t.session_key = s.session_key)
                 FROM sessions s ORDER BY s.updated_at DESC",
            )
            .context("prepare session usage")?;
        let rows = statement
            .query_map([], |row| {
                Ok(SessionUsage {
                    session: row.get(0)?,
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
                        hit_percent: 0.0,
                    },
                })
            })
            .context("read session usage")?;
        let mut usage = Vec::new();
        for row in rows {
            let mut row = row.context("read session usage row")?;
            row.cache.hit_percent =
                cache_hit_percent(row.cache.cached_tokens, row.cache.input_tokens);
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

    pub fn export(&self, key: &str) -> Result<Value> {
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
        let history = self.history(key)?;
        let mut items = Vec::new();
        for entry in &history {
            items.push(json!({
                "position": entry.position,
                "reference": entry.reference,
                "item": self.message_json(key, &entry.reference)?.map(|raw| serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null)),
            }));
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
        Ok(json!({
            "session": key,
            "revision": session.revision,
            "history": items,
            "segments": segments,
            "blocks": blocks,
        }))
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
