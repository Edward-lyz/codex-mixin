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
const TURN_STATUS_COLUMNS: [(&str, &str); 4] = [
    ("status", "TEXT NOT NULL DEFAULT 'completed'"),
    ("error", "TEXT"),
    ("estimated_cost_usd", "REAL"),
    ("client", "TEXT"),
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
    /// Authenticated gateway client (the API key's name) that sent the
    /// request, so usage can be read per key as well as per provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) client: Option<String>,
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

/// Session id for requests that carry no conversation identity. They are
/// grouped under one label so the usage page still lists them.
pub(crate) const ANONYMOUS_SESSION: &str = "anonymous-api";

/// Label the usage page shows for [`ANONYMOUS_SESSION`] rows.
pub(crate) const ANONYMOUS_SESSION_LABEL: &str = "API";

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

/// Time window for bucketed traffic series queries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UsageSeriesWindow {
    TenMinutes,
    OneHour,
    Day,
    Week,
    Month,
    NinetyDays,
}

impl UsageSeriesWindow {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "10m" => Some(Self::TenMinutes),
            "1h" => Some(Self::OneHour),
            "24h" => Some(Self::Day),
            "7d" => Some(Self::Week),
            "30d" => Some(Self::Month),
            "90d" => Some(Self::NinetyDays),
            _ => None,
        }
    }

    pub(crate) const fn bucket_ms(self) -> u64 {
        match self {
            Self::TenMinutes => 20_000,
            Self::OneHour => 120_000,
            Self::Day => 3_600_000,
            Self::Week | Self::Month | Self::NinetyDays => DAY_MS,
        }
    }

    pub(crate) const fn bucket_count(self) -> usize {
        match self {
            Self::TenMinutes | Self::OneHour | Self::Month => 30,
            Self::Day => 24,
            Self::Week => 7,
            Self::NinetyDays => 90,
        }
    }

    pub(crate) const fn days(self) -> Option<u64> {
        match self {
            Self::Week => Some(7),
            Self::Month => Some(30),
            Self::NinetyDays => Some(90),
            Self::TenMinutes | Self::OneHour | Self::Day => None,
        }
    }
}

/// One aggregated time bucket returned by `/v1/usage/series` and `usage --series`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct UsageSeriesBucket {
    pub(crate) bucket_start_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provider_id: Option<String>,
    pub(crate) requests: u64,
    pub(crate) errors: u64,
    pub(crate) cached_requests: u64,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) estimated_cost_usd: f64,
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

    /// Bucketed counts from the turn ledger for short windows (`10m`, `1h`, `24h`).
    pub(crate) fn series(
        &self,
        window: UsageSeriesWindow,
        provider_id: Option<&str>,
        by_provider: bool,
        providers: Option<&HashSet<String>>,
    ) -> Vec<UsageSeriesBucket> {
        let now_ms = unix_millis();
        let bucket_ms = window.bucket_ms();
        let count = window.bucket_count();
        let current_bucket = (now_ms / bucket_ms) * bucket_ms;
        let start_ms =
            current_bucket.saturating_sub(bucket_ms.saturating_mul(count.saturating_sub(1) as u64));
        let end_ms = current_bucket.saturating_add(bucket_ms);
        let mut buckets: Vec<UsageSeriesBucket> = if by_provider {
            Vec::new()
        } else {
            (0..count)
                .map(|index| UsageSeriesBucket {
                    bucket_start_ms: start_ms
                        .saturating_add(bucket_ms.saturating_mul(index as u64)),
                    ..UsageSeriesBucket::default()
                })
                .collect()
        };
        let Some(path) = self.persist_path.as_deref().filter(|path| path.exists()) else {
            return buckets;
        };
        match query_series(
            path,
            start_ms,
            end_ms,
            bucket_ms,
            provider_id,
            by_provider,
            providers,
        ) {
            Ok(rows) => {
                if by_provider {
                    return rows;
                }
                for row in rows {
                    if row.bucket_start_ms >= start_ms {
                        let index = ((row.bucket_start_ms - start_ms) / bucket_ms) as usize;
                        if let Some(slot) = buckets.get_mut(index) {
                            *slot = row;
                        }
                    }
                }
                buckets
            }
            Err(error) => {
                tracing::warn!(error = %error, "failed to read usage series");
                buckets
            }
        }
    }
}

fn query_series(
    path: &Path,
    start_ms: u64,
    end_ms: u64,
    bucket_ms: u64,
    provider_id: Option<&str>,
    by_provider: bool,
    providers: Option<&HashSet<String>>,
) -> anyhow::Result<Vec<UsageSeriesBucket>> {
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
    let columns = turn_columns(&connection)?;
    let has = |name: &str| columns.iter().any(|column| column == name);
    let status_expr = if has("status") {
        "status"
    } else {
        "'completed'"
    };
    let cost_expr = if has("estimated_cost_usd") {
        "COALESCE(estimated_cost_usd, 0.0)"
    } else {
        "0.0"
    };
    let mut statement = connection.prepare(&format!(
        "SELECT ((at_ms / ?1) * ?1) AS bucket_start_ms,
                provider_id,
                SUM(CASE WHEN {status_expr} = 'failed' THEN 0 ELSE 1 END) AS requests,
                SUM(CASE WHEN {status_expr} = 'failed' THEN 1 ELSE 0 END) AS errors,
                SUM(CASE WHEN COALESCE(cache_read_tokens, 0) > 0 THEN 1 ELSE 0 END) AS cached_requests,
                SUM(COALESCE(input_tokens, 0)) AS input_tokens,
                SUM(COALESCE(output_tokens, 0)) AS output_tokens,
                SUM(COALESCE(cache_read_tokens, 0)) AS cache_read_tokens,
                SUM({cost_expr}) AS estimated_cost_usd
         FROM token_usage_turns
         WHERE at_ms >= ?2 AND at_ms < ?3 AND (?4 IS NULL OR provider_id = ?4)
         GROUP BY bucket_start_ms, provider_id
         ORDER BY bucket_start_ms ASC, provider_id ASC"
    ))?;
    let mut rows = statement.query(params![
        bucket_ms.min(i64::MAX as u64) as i64,
        start_ms.min(i64::MAX as u64) as i64,
        end_ms.min(i64::MAX as u64) as i64,
        provider_id,
    ])?;
    let mut merged = std::collections::BTreeMap::<(u64, Option<String>), UsageSeriesBucket>::new();
    while let Some(row) = rows.next()? {
        let bucket_start_ms: u64 = row.get::<_, i64>(0)?.max(0) as u64;
        let row_provider: String = row.get(1)?;
        if providers.is_some_and(|allowed| !allowed.contains(&row_provider)) {
            continue;
        }
        let key_provider = by_provider.then_some(row_provider);
        let entry = merged
            .entry((bucket_start_ms, key_provider.clone()))
            .or_insert_with(|| UsageSeriesBucket {
                bucket_start_ms,
                provider_id: key_provider,
                ..UsageSeriesBucket::default()
            });
        entry.requests = entry
            .requests
            .saturating_add(row.get::<_, i64>(2)?.max(0) as u64);
        entry.errors = entry
            .errors
            .saturating_add(row.get::<_, i64>(3)?.max(0) as u64);
        entry.cached_requests = entry
            .cached_requests
            .saturating_add(row.get::<_, i64>(4)?.max(0) as u64);
        entry.input_tokens = entry
            .input_tokens
            .saturating_add(row.get::<_, i64>(5)?.max(0) as u64);
        entry.output_tokens = entry
            .output_tokens
            .saturating_add(row.get::<_, i64>(6)?.max(0) as u64);
        entry.cache_read_tokens = entry
            .cache_read_tokens
            .saturating_add(row.get::<_, i64>(7)?.max(0) as u64);
        entry.estimated_cost_usd += row.get::<_, f64>(8)?;
    }
    Ok(merged.into_values().collect())
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
    let client_column = if has("client") { "client" } else { "NULL" };
    // Inserts run on the blocking pool and may land out of order, so order by
    // the reply timestamp rather than the row id.
    let mut statement = connection.prepare(&format!(
        "SELECT at_ms, session, provider_id, catalog_slug, upstream_model_id, returned_model,
                protocol, prefix_state, input_tokens, cache_read_tokens, cache_creation_tokens,
                output_tokens, reasoning_tokens, reasoning_chars, ttft_ms, output_tps,
                {status_columns}, {cost_column}, {client_column}
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
                client: row.get(19)?,
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
            estimated_cost_usd, client
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                   ?19, ?20)",
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
            turn.client,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_db_path(label: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "codex-mixin-usage-series-{label}-{}-{id}.sqlite",
            std::process::id()
        ))
    }

    fn sample_turn(
        at_ms: i64,
        provider_id: &str,
        cache_read_tokens: u64,
        status: TurnStatus,
        cost: Option<f64>,
    ) -> TurnUsage {
        TurnUsage {
            at_ms: at_ms as u64,
            session: "sess-1".to_string(),
            session_display: None,
            provider_id: provider_id.to_string(),
            catalog_slug: "gpt-5".to_string(),
            upstream_model_id: "gpt-5".to_string(),
            returned_model: None,
            model_matches: Some(true),
            protocol: "openai_responses".to_string(),
            prefix_state: "hit".to_string(),
            input_tokens: Some(100),
            cache_read_tokens: Some(cache_read_tokens),
            cache_creation_tokens: Some(0),
            output_tokens: Some(50),
            reasoning_tokens: Some(0),
            reasoning_chars: 0,
            cache_hit_percent: None,
            ttft_ms: Some(120.0),
            output_tps: Some(40.0),
            status,
            error: (status == TurnStatus::Failed).then(|| "boom".to_string()),
            estimated_cost_usd: cost,
            client: Some("codex".to_string()),
        }
    }

    #[tokio::test]
    async fn series_zero_fills_buckets_and_aggregates_requests_cached_errors() {
        let path = temp_db_path("agg");
        let ledger = TurnLedger::load(Some(path.clone())).expect("load temp ledger");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let bucket_ms = UsageSeriesWindow::TenMinutes.bucket_ms() as i64;
        let current_slot = now_ms - now_ms.rem_euclid(bucket_ms);
        let prev_slot = current_slot - bucket_ms;

        ledger.record(sample_turn(
            current_slot + 100,
            "openai",
            64,
            TurnStatus::Completed,
            Some(0.25),
        ));
        ledger.record(sample_turn(
            current_slot + 200,
            "anthropic",
            0,
            TurnStatus::Failed,
            Some(0.10),
        ));
        ledger.record(sample_turn(
            prev_slot + 50,
            "openai",
            32,
            TurnStatus::Completed,
            Some(0.50),
        ));
        let mut buckets = Vec::new();
        for _ in 0..50 {
            buckets = ledger.series(UsageSeriesWindow::TenMinutes, None, false, None);
            let total: u64 = buckets
                .iter()
                .map(|bucket| bucket.requests + bucket.errors)
                .sum();
            if total == 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        assert_eq!(buckets.len(), UsageSeriesWindow::TenMinutes.bucket_count());
        let total_requests: u64 = buckets.iter().map(|bucket| bucket.requests).sum();
        let total_cached: u64 = buckets.iter().map(|bucket| bucket.cached_requests).sum();
        let total_errors: u64 = buckets.iter().map(|bucket| bucket.errors).sum();
        let total_cost: f64 = buckets.iter().map(|bucket| bucket.estimated_cost_usd).sum();
        assert_eq!(total_requests, 2);
        assert_eq!(total_cached, 2);
        assert_eq!(total_errors, 1);
        assert!((total_cost - 0.85).abs() < 1e-6);

        let by_provider = ledger.series(UsageSeriesWindow::TenMinutes, None, true, None);
        let openai_requests: u64 = by_provider
            .iter()
            .filter(|bucket| bucket.provider_id.as_deref() == Some("openai"))
            .map(|bucket| bucket.requests)
            .sum();
        let anthropic_errors: u64 = by_provider
            .iter()
            .filter(|bucket| bucket.provider_id.as_deref() == Some("anthropic"))
            .map(|bucket| bucket.errors)
            .sum();
        assert_eq!(openai_requests, 2);
        assert_eq!(anthropic_errors, 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn series_window_parses_ninety_days() {
        let window = UsageSeriesWindow::parse("90d").expect("parse 90d");
        assert_eq!(window, UsageSeriesWindow::NinetyDays);
        assert_eq!(window.days(), Some(90));
        assert_eq!(window.bucket_count(), 90);
        assert_eq!(window.bucket_ms(), DAY_MS);
    }
}
