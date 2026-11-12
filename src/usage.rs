//! Per-turn usage ledger: one row per upstream model reply.
//!
//! The provider/model aggregates answer "how much"; this ledger answers "which
//! reply": cache hits per turn, reasoning length, and whether the upstream
//! answered with the model that was requested. It keeps counters and model
//! names only, never prompt or completion text.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;

/// Rows kept on disk, about a month of busy use. Older turns stay in the
/// daily aggregates. Queries read the rows on demand, so the cap costs disk only.
pub(crate) const MAX_TRACKED_TURNS: usize = 100_000;

/// Largest page one turn query may return.
pub(crate) const MAX_TURN_PAGE: usize = 5_000;

pub(crate) const PERSIST_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Serialises writers and keeps WAL open between replies. Opening and closing
/// the last connection per write forces repeated checkpoints and filesystem work.
pub(crate) static USAGE_DB_WRITE: Mutex<Option<(PathBuf, Connection)>> = Mutex::new(None);

const TURNS_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS token_usage_turns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms INTEGER NOT NULL,
    session TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    catalog_slug TEXT NOT NULL,
    upstream_model_id TEXT NOT NULL,
    returned_model TEXT,
    protocol TEXT NOT NULL,
    prefix_state TEXT NOT NULL,
    input_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_creation_tokens INTEGER,
    output_tokens INTEGER,
    reasoning_tokens INTEGER,
    reasoning_chars INTEGER NOT NULL,
    ttft_ms REAL,
    output_tps REAL,
    status TEXT NOT NULL DEFAULT 'completed',
    error TEXT
);
CREATE INDEX IF NOT EXISTS token_usage_turns_at ON token_usage_turns (at_ms);";

/// Ledger columns added after the first release; older ledgers gain them on the
/// next write and read them as completed replies until then.
const TURN_STATUS_COLUMNS: [(&str, &str); 3] = [
    ("status", "TEXT NOT NULL DEFAULT 'completed'"),
    ("error", "TEXT"),
    ("estimated_cost_usd", "REAL"),
];

/// Longest upstream failure summary kept per reply.
const MAX_TURN_ERROR_CHARS: usize = 200;

/// Outcome of one upstream reply.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TurnStatus {
    #[default]
    Completed,
    Failed,
}

impl TurnStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    fn from_column(value: &str) -> Self {
        if value == Self::Failed.as_str() {
            Self::Failed
        } else {
            Self::Completed
        }
    }
}

/// One upstream reply. Optional counters stay `None` when the provider did not
/// report them, so unknown values are never shown as zero.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct TurnUsage {
    pub(crate) at_ms: u64,
    /// Short hash of the client session, so turns of one conversation group
    /// together without exposing the session identifier.
    pub(crate) session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_display: Option<String>,
    pub(crate) provider_id: String,
    pub(crate) catalog_slug: String,
    pub(crate) upstream_model_id: String,
    pub(crate) returned_model: Option<String>,
    pub(crate) model_matches: Option<bool>,
    pub(crate) protocol: String,
    pub(crate) prefix_state: String,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) cache_read_tokens: Option<u64>,
    pub(crate) cache_creation_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) reasoning_tokens: Option<u64>,
    /// Characters of streamed reasoning text; Anthropic-style upstreams stream
    /// thinking without a separate token counter.
    pub(crate) reasoning_chars: u64,
    pub(crate) cache_hit_percent: Option<f64>,
    pub(crate) ttft_ms: Option<f64>,
    pub(crate) output_tps: Option<f64>,
    pub(crate) status: TurnStatus,
    /// List-price USD of this reply, None when its model has no price or it
    /// reported no counters.
    pub(crate) estimated_cost_usd: Option<f64>,
    /// Short upstream failure summary, such as `HTTP 429`, for failed replies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

impl TurnUsage {
    pub(crate) fn finalize(mut self) -> Self {
        if let Some(error) = &mut self.error
            && let Some((cut, _)) = error.char_indices().nth(MAX_TURN_ERROR_CHARS)
        {
            error.truncate(cut);
        }
        self.model_matches = self
            .returned_model
            .as_deref()
            .map(|returned| model_matches(&self.upstream_model_id, returned));
        self.cache_hit_percent = match (self.cache_read_tokens, self.input_tokens) {
            (Some(read), Some(uncached)) => {
                let prompt = read
                    .saturating_add(uncached)
                    .saturating_add(self.cache_creation_tokens.unwrap_or(0));
                (prompt > 0).then(|| read as f64 / prompt as f64 * 100.0)
            }
            _ => None,
        };
        self
    }
}

pub(crate) fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

const DAY_MS: u64 = 86_400_000;

/// Start of the earliest UTC day in a `days`-long range ending today; the
/// daily aggregates bucket by the same UTC days.
pub(crate) fn first_day_start_ms(days: u64) -> u64 {
    let today = unix_millis() / DAY_MS;
    today.saturating_sub(days.saturating_sub(1)) * DAY_MS
}

pub(crate) fn session_tag(session_id: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}

/// Latest ledger row for one client session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionTurn {
    pub(crate) provider_id: String,
    pub(crate) catalog_slug: String,
}

/// Reads the newest turn recorded for `session_id` from the ledger under
/// `state_dir`.
///
/// The store owns the database name, schema, and session-tag derivation so
/// callers never open SQLite or depend on the table layout. A missing ledger or
/// row is `Ok(None)`; an unreadable or malformed ledger is an error the caller
/// may downgrade to a skipped fallback.
pub(crate) fn latest_session_turn(
    state_dir: &Path,
    session_id: &str,
) -> anyhow::Result<Option<SessionTurn>> {
    let path = state_dir.join("usage.sqlite3");
    if !path.is_file() {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open usage ledger {}", path.display()))?;
    let has_table: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'token_usage_turns')",
            [],
            |row| row.get(0),
        )
        .with_context(|| format!("inspect usage ledger {}", path.display()))?;
    if !has_table {
        return Ok(None);
    }
    let tag = session_tag(session_id);
    let row = connection.query_row(
        "SELECT provider_id, catalog_slug FROM token_usage_turns \
         WHERE session = ?1 ORDER BY at_ms DESC LIMIT 1",
        [&tag],
        |row| {
            Ok(SessionTurn {
                provider_id: row.get(0)?,
                catalog_slug: row.get(1)?,
            })
        },
    );
    match row {
        Ok(turn) => Ok(Some(turn)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("query usage ledger {}", path.display())),
    }
}

/// Providers answer with namespaced or dated snapshots of the requested model
/// ("openai/gpt-5", "gpt-5-2025-08-07", "claude-sonnet-4-6-20260101"), and
/// relays list models under a short alias of the vendor id ("Opus 5.5" for
/// "claude-opus-5-5"). Only those variants match: a sibling such as
/// "gpt-5.4-mini" for "gpt-5.4" does not, because it is a different model.
pub(crate) fn model_matches(requested: &str, returned: &str) -> bool {
    let normalize = |name: &str| {
        let name = name.rsplit('/').next().unwrap_or(name);
        let name = name
            .trim()
            .to_ascii_lowercase()
            .replace(['_', '.', ' '], "-");
        strip_date_suffix(&name).to_owned()
    };
    let (requested, returned) = (normalize(requested), normalize(returned));
    if returned.is_empty() {
        return false;
    }
    requested == returned
        || is_vendor_alias(&requested, &returned)
        || is_vendor_alias(&returned, &requested)
}

/// `alias` is `full` without one leading vendor word ("opus-5-5" for
/// "claude-opus-5-5"). The alias must still name a version, so a bare family
/// word such as "mini" never matches.
fn is_vendor_alias(alias: &str, full: &str) -> bool {
    let Some(vendor) = full
        .strip_suffix(alias)
        .and_then(|rest| rest.strip_suffix('-'))
    else {
        return false;
    };
    !vendor.is_empty()
        && vendor.bytes().all(|byte| byte.is_ascii_alphabetic())
        && alias.contains('-')
        && alias.bytes().any(|byte| byte.is_ascii_digit())
        && alias.bytes().any(|byte| byte.is_ascii_alphabetic())
}

/// Removes a trailing snapshot date: `-YYYYMMDD`, `-YYYY-MM-DD`, or `-MMDD`.
fn strip_date_suffix(name: &str) -> &str {
    let digits = |part: &str, len: usize| {
        part.len() == len && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    let parts: Vec<&str> = name.rsplitn(4, '-').collect();
    // rsplitn yields the tail first: ["DD", "MM", "YYYY", rest].
    if parts.len() == 4 && digits(parts[0], 2) && digits(parts[1], 2) && digits(parts[2], 4) {
        return parts[3];
    }
    match name.rsplit_once('-') {
        Some((rest, tail)) if digits(tail, 8) || digits(tail, 4) => rest,
        _ => name,
    }
}

/// Filters for a ledger query; `limit` is applied after filtering.
#[derive(Debug, Default)]
pub(crate) struct TurnQuery {
    pub(crate) since_ms: Option<u64>,
    /// Exclusive upper bound on the reply time.
    pub(crate) until_ms: Option<u64>,
    pub(crate) provider_id: Option<String>,
    pub(crate) limit: usize,
}

/// Turn rows live only in usage.sqlite3; queries read them on demand so the
/// gateway does not keep thousands of rows resident between dashboard refreshes.
#[derive(Debug, Default)]
pub(crate) struct TurnLedger {
    persist_path: Option<PathBuf>,
}

impl TurnLedger {
    pub(crate) fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        Ok(Self { persist_path: path })
    }

    pub(crate) fn record(&self, turn: TurnUsage) {
        let Some(path) = self.persist_path.clone() else {
            return;
        };
        tokio::task::spawn_blocking(move || {
            if let Err(error) = persist_turn(&path, &turn) {
                tracing::warn!(error = %error, "failed to persist turn usage");
            }
        });
    }

    /// Newest turns first.
    pub(crate) fn query(
        &self,
        query: &TurnQuery,
        providers: Option<&HashSet<String>>,
    ) -> Vec<TurnUsage> {
        let Some(path) = self.persist_path.as_deref().filter(|path| path.exists()) else {
            return Vec::new();
        };
        match query_turns(path, query, providers) {
            Ok(turns) => turns,
            Err(error) => {
                tracing::warn!(error = %error, "failed to read turn usage");
                Vec::new()
            }
        }
    }
}

fn query_turns(
    path: &Path,
    query: &TurnQuery,
    providers: Option<&HashSet<String>>,
) -> anyhow::Result<Vec<TurnUsage>> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(PERSIST_BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA cache_size = -256;")?;
    let table_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'token_usage_turns')",
        [],
        |row| row.get(0),
    )?;
    if !table_exists {
        return Ok(Vec::new());
    }
    // The reader is read-only, so a ledger last written by an older build
    // reports every row as completed instead of failing the query.
    let columns = turn_columns(&connection)?;
    let has = |name: &str| columns.iter().any(|column| column == name);
    let status_columns = if has("status") {
        "status, error"
    } else {
        "'completed', NULL"
    };
    let cost_column = if has("estimated_cost_usd") {
        "estimated_cost_usd"
    } else {
        "NULL"
    };
    // Inserts run on the blocking pool and may land out of order, so order by
    // the reply timestamp rather than the row id.
    let mut statement = connection.prepare(&format!(
        "SELECT at_ms, session, provider_id, catalog_slug, upstream_model_id, returned_model,
                protocol, prefix_state, input_tokens, cache_read_tokens, cache_creation_tokens,
                output_tokens, reasoning_tokens, reasoning_chars, ttft_ms, output_tps,
                {status_columns}, {cost_column}
         FROM token_usage_turns
         WHERE (?1 IS NULL OR at_ms >= ?1) AND (?2 IS NULL OR provider_id = ?2)
               AND (?3 IS NULL OR at_ms < ?3)
         ORDER BY at_ms DESC, id DESC"
    ))?;
    let since = query
        .since_ms
        .map(|since| since.min(i64::MAX as u64) as i64);
    let until = query
        .until_ms
        .map(|until| until.min(i64::MAX as u64) as i64);
    let mut rows = statement.query(params![since, query.provider_id, until])?;
    let mut turns = Vec::new();
    while turns.len() < query.limit {
        let Some(row) = rows.next()? else {
            break;
        };
        let provider_id: String = row.get(2)?;
        if providers.is_some_and(|providers| !providers.contains(&provider_id)) {
            continue;
        }
        turns.push(
            TurnUsage {
                at_ms: row.get(0)?,
                session: row.get(1)?,
                session_display: None,
                provider_id,
                catalog_slug: row.get(3)?,
                upstream_model_id: row.get(4)?,
                returned_model: row.get(5)?,
                model_matches: None,
                protocol: row.get(6)?,
                prefix_state: row.get(7)?,
                input_tokens: row.get(8)?,
                cache_read_tokens: row.get(9)?,
                cache_creation_tokens: row.get(10)?,
                output_tokens: row.get(11)?,
                reasoning_tokens: row.get(12)?,
                reasoning_chars: row.get(13)?,
                cache_hit_percent: None,
                ttft_ms: row.get(14)?,
                output_tps: row.get(15)?,
                status: TurnStatus::from_column(&row.get::<_, String>(16)?),
                estimated_cost_usd: row.get(18)?,
                error: row.get(17)?,
            }
            .finalize(),
        );
    }
    Ok(turns)
}

fn persist_turn(path: &Path, turn: &TurnUsage) -> anyhow::Result<()> {
    let mut write = USAGE_DB_WRITE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if write.as_ref().is_none_or(|(current, _)| current != path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        *write = Some((path.to_path_buf(), Connection::open(path)?));
    }
    let Some((_, connection)) = write.as_mut() else {
        anyhow::bail!("usage database connection is unavailable");
    };
    // Turn rows and the aggregate counters are written from concurrent
    // blocking tasks; wait for the writer lock instead of dropping the row.
    connection.busy_timeout(PERSIST_BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
    connection.execute_batch(TURNS_SCHEMA)?;
    let columns = turn_columns(connection)?;
    for (column, definition) in TURN_STATUS_COLUMNS {
        // Another process may add the column between the check and ALTER.
        if !columns.iter().any(|existing| existing == column)
            && let Err(error) = connection.execute(
                &format!("ALTER TABLE token_usage_turns ADD COLUMN {column} {definition}"),
                [],
            )
            && !turn_columns(connection)?
                .iter()
                .any(|existing| existing == column)
        {
            return Err(error.into());
        }
    }
    connection.execute(
        "INSERT INTO token_usage_turns (
            at_ms, session, provider_id, catalog_slug, upstream_model_id, returned_model,
            protocol, prefix_state, input_tokens, cache_read_tokens, cache_creation_tokens,
            output_tokens, reasoning_tokens, reasoning_chars, ttft_ms, output_tps, status, error,
            estimated_cost_usd
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                   ?19)",
        params![
            turn.at_ms,
            turn.session,
            turn.provider_id,
            turn.catalog_slug,
            turn.upstream_model_id,
            turn.returned_model,
            turn.protocol,
            turn.prefix_state,
            turn.input_tokens,
            turn.cache_read_tokens,
            turn.cache_creation_tokens,
            turn.output_tokens,
            turn.reasoning_tokens,
            turn.reasoning_chars,
            turn.ttft_ms,
            turn.output_tps,
            turn.status.as_str(),
            turn.error,
            turn.estimated_cost_usd,
        ],
    )?;
    connection.execute(
        "DELETE FROM token_usage_turns WHERE id <= last_insert_rowid() - ?1",
        [MAX_TRACKED_TURNS as i64],
    )?;
    Ok(())
}

fn turn_columns(connection: &Connection) -> anyhow::Result<Vec<String>> {
    let mut statement = connection.prepare("PRAGMA table_info(token_usage_turns)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns)
}
