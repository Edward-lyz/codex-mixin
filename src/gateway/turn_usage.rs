//! Per-turn usage ledger: one row per upstream model reply.
//!
//! The provider/model aggregates answer "how much"; this ledger answers "which
//! reply": cache hits per turn, reasoning length, and whether the upstream
//! answered with the model that was requested. It keeps counters and model
//! names only, never prompt or completion text.

use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde::Serialize;

/// Rows kept in memory and on disk. At a few hundred bytes per row this bounds
/// the ledger to about 2 MB; older turns stay in the daily aggregates.
pub(crate) const MAX_TRACKED_TURNS: usize = 5_000;

const PERSIST_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Serialises writers to `usage.sqlite3` inside this process. Each reply
/// persists from its own blocking task, and SQLite reports "database is
/// locked" instead of waiting while a fresh file switches to WAL.
pub(super) static USAGE_DB_WRITE: Mutex<()> = Mutex::new(());

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
    output_tps REAL
)";

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
}

impl TurnUsage {
    pub(crate) fn finalize(mut self) -> Self {
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

/// Providers answer with namespaced or dated snapshots of the requested model
/// ("openai/gpt-5", "gpt-5-2025-08-07", "claude-sonnet-4-6-20260101"). Only
/// those variants match: a sibling such as "gpt-5.4-mini" for "gpt-5.4" does
/// not, because it is a different model.
pub(crate) fn model_matches(requested: &str, returned: &str) -> bool {
    let normalize = |name: &str| {
        let name = name.rsplit('/').next().unwrap_or(name);
        let name = name.to_ascii_lowercase().replace(['_', '.', ' '], "-");
        strip_date_suffix(&name).to_owned()
    };
    let (requested, returned) = (normalize(requested), normalize(returned));
    !returned.is_empty() && requested == returned
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
    pub(crate) provider_id: Option<String>,
    pub(crate) limit: usize,
}

#[derive(Debug, Default)]
pub(crate) struct TurnLedger {
    turns: Mutex<VecDeque<TurnUsage>>,
    persist_path: Option<PathBuf>,
}

impl TurnLedger {
    pub(crate) fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let turns = match path.as_deref() {
            Some(path) if path.exists() => load_turns(path)?,
            _ => VecDeque::new(),
        };
        Ok(Self {
            turns: Mutex::new(turns),
            persist_path: path,
        })
    }

    pub(crate) fn record(&self, turn: TurnUsage) {
        {
            let mut turns = self.turns.lock().unwrap_or_else(PoisonError::into_inner);
            if turns.len() == MAX_TRACKED_TURNS {
                turns.pop_front();
            }
            turns.push_back(turn.clone());
        }
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
        let turns = self.turns.lock().unwrap_or_else(PoisonError::into_inner);
        turns
            .iter()
            .rev()
            .filter(|turn| query.since_ms.is_none_or(|since| turn.at_ms >= since))
            .filter(|turn| providers.is_none_or(|providers| providers.contains(&turn.provider_id)))
            .filter(|turn| {
                query
                    .provider_id
                    .as_deref()
                    .is_none_or(|provider| provider == turn.provider_id)
            })
            .take(query.limit)
            .cloned()
            .collect()
    }
}

fn load_turns(path: &Path) -> anyhow::Result<VecDeque<TurnUsage>> {
    let connection = Connection::open(path)?;
    connection.execute_batch(TURNS_SCHEMA)?;
    let mut statement = connection.prepare(
        "SELECT at_ms, session, provider_id, catalog_slug, upstream_model_id, returned_model,
                protocol, prefix_state, input_tokens, cache_read_tokens, cache_creation_tokens,
                output_tokens, reasoning_tokens, reasoning_chars, ttft_ms, output_tps
         FROM (SELECT * FROM token_usage_turns ORDER BY id DESC LIMIT ?1)
         ORDER BY at_ms, id",
    )?;
    // Inserts run on the blocking pool and may land out of order, so restore
    // reply order from the timestamp rather than the row id.
    let rows = statement.query_map([MAX_TRACKED_TURNS as i64], |row| {
        Ok(TurnUsage {
            at_ms: row.get(0)?,
            session: row.get(1)?,
            session_display: None,
            provider_id: row.get(2)?,
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
        }
        .finalize())
    })?;
    Ok(rows.collect::<Result<VecDeque<_>, _>>()?)
}

fn persist_turn(path: &Path, turn: &TurnUsage) -> anyhow::Result<()> {
    let _write = USAGE_DB_WRITE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let connection = Connection::open(path)?;
    // Turn rows and the aggregate counters are written from concurrent
    // blocking tasks; wait for the writer lock instead of dropping the row.
    connection.busy_timeout(PERSIST_BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
    connection.execute_batch(TURNS_SCHEMA)?;
    connection.execute(
        "INSERT INTO token_usage_turns (
            at_ms, session, provider_id, catalog_slug, upstream_model_id, returned_model,
            protocol, prefix_state, input_tokens, cache_read_tokens, cache_creation_tokens,
            output_tokens, reasoning_tokens, reasoning_chars, ttft_ms, output_tps
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
        ],
    )?;
    connection.execute(
        "DELETE FROM token_usage_turns WHERE id <= last_insert_rowid() - ?1",
        [MAX_TRACKED_TURNS as i64],
    )?;
    Ok(())
}
