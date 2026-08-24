//! Provider usage accounting and persistence for prompt-cache diagnostics.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::Value;

/// Provider-level token and prompt cache counters observed on upstream
/// responses, kept compact so the menu can visualize usage without retaining
/// request bodies or history.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct ProviderTokenUsage {
    pub(crate) provider_id: String,
    pub(crate) model_id: String,
    pub(crate) request_count: u64,
    pub(crate) input_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_hit_percent: Option<f64>,
    pub(crate) average_ttft_ms: Option<f64>,
    pub(crate) output_tps: Option<f64>,
    #[serde(skip)]
    pub(crate) observed_cache_read_tokens: u64,
    #[serde(skip)]
    pub(crate) observed_uncached_input_tokens: u64,
    #[serde(skip)]
    pub(crate) timing_sample_count: u64,
    #[serde(skip)]
    pub(crate) total_ttft_micros: u64,
    #[serde(skip)]
    pub(crate) total_generation_micros: u64,
    #[serde(skip)]
    pub(crate) timed_output_tokens: u64,
    #[serde(skip)]
    pub(crate) total_output_tps: f64,
    #[serde(skip)]
    pub(crate) tps_sample_count: u64,
}

#[derive(Debug, Default)]
struct TokenUsageState {
    entries: HashMap<(String, String), ProviderTokenUsage>,
    daily_entries: HashMap<(u64, String, String), ProviderTokenUsage>,
}

fn current_unix_day() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() / 86_400)
}

fn ensure_timing_columns(connection: &Connection, table: &str) -> anyhow::Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (column, column_type) in [
        ("timing_sample_count", "INTEGER"),
        ("total_ttft_micros", "INTEGER"),
        ("total_generation_micros", "INTEGER"),
        ("timed_output_tokens", "INTEGER"),
        ("total_output_tps", "REAL"),
        ("tps_sample_count", "INTEGER"),
    ] {
        if !columns.iter().any(|existing| existing == column) {
            connection.execute(
                &format!(
                    "ALTER TABLE {table} ADD COLUMN {column} {column_type} NOT NULL DEFAULT 0"
                ),
                [],
            )?;
        }
    }
    Ok(())
}

fn load_persisted_usage(path: &Path) -> anyhow::Result<TokenUsageState> {
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS token_usage (
            provider_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            request_count INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL,
            cache_read_tokens INTEGER NOT NULL,
            cache_creation_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            observed_cache_read_tokens INTEGER NOT NULL,
            observed_uncached_input_tokens INTEGER NOT NULL,
            timing_sample_count INTEGER NOT NULL DEFAULT 0,
            total_ttft_micros INTEGER NOT NULL DEFAULT 0,
            total_generation_micros INTEGER NOT NULL DEFAULT 0,
            timed_output_tokens INTEGER NOT NULL DEFAULT 0,
            total_output_tps REAL NOT NULL DEFAULT 0,
            tps_sample_count INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (provider_id, model_id)
        );
        CREATE TABLE IF NOT EXISTS token_usage_daily (
            day INTEGER NOT NULL,
            provider_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            request_count INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL,
            cache_read_tokens INTEGER NOT NULL,
            cache_creation_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            observed_cache_read_tokens INTEGER NOT NULL,
            observed_uncached_input_tokens INTEGER NOT NULL,
            timing_sample_count INTEGER NOT NULL DEFAULT 0,
            total_ttft_micros INTEGER NOT NULL DEFAULT 0,
            total_generation_micros INTEGER NOT NULL DEFAULT 0,
            timed_output_tokens INTEGER NOT NULL DEFAULT 0,
            total_output_tps REAL NOT NULL DEFAULT 0,
            tps_sample_count INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (day, provider_id, model_id)
        )",
    )?;
    ensure_timing_columns(&connection, "token_usage")?;
    ensure_timing_columns(&connection, "token_usage_daily")?;
    let mut statement = connection.prepare(
        "SELECT provider_id, model_id, request_count, input_tokens, cache_read_tokens,
                cache_creation_tokens, output_tokens, observed_cache_read_tokens,
                observed_uncached_input_tokens, timing_sample_count, total_ttft_micros,
                total_generation_micros, timed_output_tokens, total_output_tps, tps_sample_count
         FROM token_usage",
    )?;
    let rows = statement.query_map([], |row| {
        let provider_id = row.get::<_, String>(0)?;
        let model_id = row.get::<_, String>(1)?;
        Ok((
            (provider_id.clone(), model_id.clone()),
            ProviderTokenUsage {
                provider_id,
                model_id,
                request_count: row.get(2)?,
                input_tokens: row.get(3)?,
                cache_read_tokens: row.get(4)?,
                cache_creation_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                cache_hit_percent: None,
                average_ttft_ms: None,
                output_tps: None,
                observed_cache_read_tokens: row.get(7)?,
                observed_uncached_input_tokens: row.get(8)?,
                timing_sample_count: row.get(9)?,
                total_ttft_micros: row.get(10)?,
                total_generation_micros: row.get(11)?,
                timed_output_tokens: row.get(12)?,
                total_output_tps: row.get(13)?,
                tps_sample_count: row.get(14)?,
            },
        ))
    })?;
    let entries = rows.collect::<Result<HashMap<_, _>, _>>()?;
    drop(statement);
    let mut daily_statement = connection.prepare(
        "SELECT day, provider_id, model_id, request_count, input_tokens, cache_read_tokens,
                cache_creation_tokens, output_tokens, observed_cache_read_tokens,
                observed_uncached_input_tokens, timing_sample_count, total_ttft_micros,
                total_generation_micros, timed_output_tokens, total_output_tps, tps_sample_count
         FROM token_usage_daily",
    )?;
    let daily_rows = daily_statement.query_map([], |row| {
        let day = row.get::<_, u64>(0)?;
        let provider_id = row.get::<_, String>(1)?;
        let model_id = row.get::<_, String>(2)?;
        Ok((
            (day, provider_id.clone(), model_id.clone()),
            ProviderTokenUsage {
                provider_id,
                model_id,
                request_count: row.get(3)?,
                input_tokens: row.get(4)?,
                cache_read_tokens: row.get(5)?,
                cache_creation_tokens: row.get(6)?,
                output_tokens: row.get(7)?,
                cache_hit_percent: None,
                average_ttft_ms: None,
                output_tps: None,
                observed_cache_read_tokens: row.get(8)?,
                observed_uncached_input_tokens: row.get(9)?,
                timing_sample_count: row.get(10)?,
                total_ttft_micros: row.get(11)?,
                total_generation_micros: row.get(12)?,
                timed_output_tokens: row.get(13)?,
                total_output_tps: row.get(14)?,
                tps_sample_count: row.get(15)?,
            },
        ))
    })?;
    Ok(TokenUsageState {
        entries,
        daily_entries: daily_rows.collect::<Result<HashMap<_, _>, _>>()?,
    })
}

fn persist_usage_delta(
    path: &Path,
    provider_id: &str,
    model_id: &str,
    usage: &UpstreamCacheUsage,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut connection = Connection::open(path)?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS token_usage (
            provider_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            request_count INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL,
            cache_read_tokens INTEGER NOT NULL,
            cache_creation_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            observed_cache_read_tokens INTEGER NOT NULL,
            observed_uncached_input_tokens INTEGER NOT NULL,
            timing_sample_count INTEGER NOT NULL DEFAULT 0,
            total_ttft_micros INTEGER NOT NULL DEFAULT 0,
            total_generation_micros INTEGER NOT NULL DEFAULT 0,
            timed_output_tokens INTEGER NOT NULL DEFAULT 0,
            total_output_tps REAL NOT NULL DEFAULT 0,
            tps_sample_count INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (provider_id, model_id)
         );
         CREATE TABLE IF NOT EXISTS token_usage_daily (
            day INTEGER NOT NULL,
            provider_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            request_count INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL,
            cache_read_tokens INTEGER NOT NULL,
            cache_creation_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            observed_cache_read_tokens INTEGER NOT NULL,
            observed_uncached_input_tokens INTEGER NOT NULL,
            timing_sample_count INTEGER NOT NULL DEFAULT 0,
            total_ttft_micros INTEGER NOT NULL DEFAULT 0,
            total_generation_micros INTEGER NOT NULL DEFAULT 0,
            timed_output_tokens INTEGER NOT NULL DEFAULT 0,
            total_output_tps REAL NOT NULL DEFAULT 0,
            tps_sample_count INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (day, provider_id, model_id)
         )",
    )?;
    ensure_timing_columns(&connection, "token_usage")?;
    ensure_timing_columns(&connection, "token_usage_daily")?;
    let day = current_unix_day()?;
    let transaction = connection.transaction()?;
    let timing_recorded = usage.ttft_micros.is_some()
        && usage.generation_micros.is_some()
        && usage.output_tokens.is_some();
    let output_tps = match (usage.output_tokens, usage.generation_micros) {
        (Some(tokens), Some(micros)) if timing_recorded && micros > 0 => {
            tokens as f64 * 1_000_000.0 / micros as f64
        }
        _ => 0.0,
    };
    let values = params![
        provider_id,
        model_id,
        1_u64,
        usage.input_tokens.unwrap_or(0),
        usage.cache_read_tokens.unwrap_or(0),
        usage.cache_creation_tokens.unwrap_or(0),
        usage.output_tokens.unwrap_or(0),
        usage.cache_read_tokens.unwrap_or(0),
        usage
            .input_tokens
            .unwrap_or(0)
            .saturating_add(usage.cache_creation_tokens.unwrap_or(0)),
        u64::from(timing_recorded),
        usage.ttft_micros.unwrap_or(0),
        usage.generation_micros.unwrap_or(0),
        usage.output_tokens.filter(|_| timing_recorded).unwrap_or(0),
        output_tps,
        u64::from(output_tps > 0.0),
    ];
    transaction.execute(
        "INSERT INTO token_usage (
            provider_id, model_id, request_count, input_tokens, cache_read_tokens,
            cache_creation_tokens, output_tokens, observed_cache_read_tokens,
            observed_uncached_input_tokens, timing_sample_count, total_ttft_micros,
            total_generation_micros, timed_output_tokens, total_output_tps, tps_sample_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(provider_id, model_id) DO UPDATE SET
            request_count = token_usage.request_count + excluded.request_count,
            input_tokens = token_usage.input_tokens + excluded.input_tokens,
            cache_read_tokens = token_usage.cache_read_tokens + excluded.cache_read_tokens,
            cache_creation_tokens = token_usage.cache_creation_tokens + excluded.cache_creation_tokens,
            output_tokens = token_usage.output_tokens + excluded.output_tokens,
            observed_cache_read_tokens = token_usage.observed_cache_read_tokens + excluded.observed_cache_read_tokens,
            observed_uncached_input_tokens = token_usage.observed_uncached_input_tokens + excluded.observed_uncached_input_tokens,
            timing_sample_count = token_usage.timing_sample_count + excluded.timing_sample_count,
            total_ttft_micros = token_usage.total_ttft_micros + excluded.total_ttft_micros,
            total_generation_micros = token_usage.total_generation_micros + excluded.total_generation_micros,
            timed_output_tokens = token_usage.timed_output_tokens + excluded.timed_output_tokens,
            total_output_tps = token_usage.total_output_tps + excluded.total_output_tps,
            tps_sample_count = token_usage.tps_sample_count + excluded.tps_sample_count",
        values,
    )?;
    transaction.execute(
        "INSERT INTO token_usage_daily (
            day, provider_id, model_id, request_count, input_tokens, cache_read_tokens,
            cache_creation_tokens, output_tokens, observed_cache_read_tokens,
            observed_uncached_input_tokens, timing_sample_count, total_ttft_micros,
            total_generation_micros, timed_output_tokens, total_output_tps, tps_sample_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(day, provider_id, model_id) DO UPDATE SET
            request_count = token_usage_daily.request_count + excluded.request_count,
            input_tokens = token_usage_daily.input_tokens + excluded.input_tokens,
            cache_read_tokens = token_usage_daily.cache_read_tokens + excluded.cache_read_tokens,
            cache_creation_tokens = token_usage_daily.cache_creation_tokens + excluded.cache_creation_tokens,
            output_tokens = token_usage_daily.output_tokens + excluded.output_tokens,
            observed_cache_read_tokens = token_usage_daily.observed_cache_read_tokens + excluded.observed_cache_read_tokens,
            observed_uncached_input_tokens = token_usage_daily.observed_uncached_input_tokens + excluded.observed_uncached_input_tokens,
            timing_sample_count = token_usage_daily.timing_sample_count + excluded.timing_sample_count,
            total_ttft_micros = token_usage_daily.total_ttft_micros + excluded.total_ttft_micros,
            total_generation_micros = token_usage_daily.total_generation_micros + excluded.total_generation_micros,
            timed_output_tokens = token_usage_daily.timed_output_tokens + excluded.timed_output_tokens,
            total_output_tps = token_usage_daily.total_output_tps + excluded.total_output_tps,
            tps_sample_count = token_usage_daily.tps_sample_count + excluded.tps_sample_count",
        params![
            day,
            provider_id,
            model_id,
            1_u64,
            usage.input_tokens.unwrap_or(0),
            usage.cache_read_tokens.unwrap_or(0),
            usage.cache_creation_tokens.unwrap_or(0),
            usage.output_tokens.unwrap_or(0),
            usage.cache_read_tokens.unwrap_or(0),
            usage.input_tokens
                .unwrap_or(0)
                .saturating_add(usage.cache_creation_tokens.unwrap_or(0)),
            u64::from(timing_recorded),
            usage.ttft_micros.unwrap_or(0),
            usage.generation_micros.unwrap_or(0),
            usage.output_tokens.filter(|_| timing_recorded).unwrap_or(0),
            output_tps,
            u64::from(output_tps > 0.0),
        ],
    )?;
    transaction.commit().map_err(Into::into)
}

/// In-memory aggregate of the counters seen on streamed provider responses.
#[derive(Debug, Default)]
pub(crate) struct TokenUsageAggregator {
    state: Mutex<TokenUsageState>,
    persist_path: Option<PathBuf>,
}

impl TokenUsageAggregator {
    pub(crate) fn try_from_default_path() -> anyhow::Result<Self> {
        let path = crate::config::stored_config_path()
            .parent()
            .map(|parent| parent.join("usage.sqlite3"));
        let state = match path.as_deref() {
            Some(path) if path.exists() => load_persisted_usage(path)?,
            _ => TokenUsageState::default(),
        };
        Ok(Self {
            state: Mutex::new(state),
            persist_path: path,
        })
    }

    pub(crate) fn record(&self, provider_id: &str, model_id: &str, usage: &UpstreamCacheUsage) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = state
            .entries
            .entry((provider_id.to_owned(), model_id.to_owned()))
            .or_default();
        add_usage(entry, provider_id, model_id, usage);
        match current_unix_day() {
            Ok(day) => {
                let daily_entry = state
                    .daily_entries
                    .entry((day, provider_id.to_owned(), model_id.to_owned()))
                    .or_default();
                add_usage(daily_entry, provider_id, model_id, usage);
            }
            Err(error) => {
                tracing::error!(error = %error, "system clock cannot represent token usage day");
            }
        }
        if let Some(path) = self.persist_path.clone() {
            let provider_id = provider_id.to_owned();
            let model_id = model_id.to_owned();
            let usage = *usage;
            tokio::task::spawn_blocking(move || {
                if let Err(error) = persist_usage_delta(&path, &provider_id, &model_id, &usage) {
                    tracing::warn!(error = %error, "failed to persist token usage");
                }
            });
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<ProviderTokenUsage> {
        let entries = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .values()
            .cloned()
            .collect::<Vec<_>>();
        finalize_usage(entries)
    }

    pub(crate) fn snapshot_for_days(&self, days: u64) -> anyhow::Result<Vec<ProviderTokenUsage>> {
        anyhow::ensure!(days > 0, "usage range must contain at least one day");
        let first_day = current_unix_day()?.saturating_sub(days - 1);
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut aggregated = HashMap::<(String, String), ProviderTokenUsage>::new();
        for ((day, provider_id, model_id), usage) in &state.daily_entries {
            if *day < first_day {
                continue;
            }
            let entry = aggregated
                .entry((provider_id.clone(), model_id.clone()))
                .or_default();
            add_aggregated_usage(entry, usage);
        }
        Ok(finalize_usage(aggregated.into_values().collect()))
    }
}

fn add_usage(
    entry: &mut ProviderTokenUsage,
    provider_id: &str,
    model_id: &str,
    usage: &UpstreamCacheUsage,
) {
    entry.provider_id = provider_id.to_owned();
    entry.model_id = model_id.to_owned();
    entry.request_count = entry.request_count.saturating_add(1);
    entry.input_tokens = entry
        .input_tokens
        .saturating_add(usage.input_tokens.unwrap_or(0));
    entry.cache_read_tokens = entry
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens.unwrap_or(0));
    entry.cache_creation_tokens = entry
        .cache_creation_tokens
        .saturating_add(usage.cache_creation_tokens.unwrap_or(0));
    entry.output_tokens = entry
        .output_tokens
        .saturating_add(usage.output_tokens.unwrap_or(0));
    if let (Some(cache_read), Some(uncached)) = (usage.cache_read_tokens, usage.input_tokens) {
        entry.observed_cache_read_tokens =
            entry.observed_cache_read_tokens.saturating_add(cache_read);
        entry.observed_uncached_input_tokens = entry
            .observed_uncached_input_tokens
            .saturating_add(uncached)
            .saturating_add(usage.cache_creation_tokens.unwrap_or(0));
    }
    if let (Some(ttft_micros), Some(generation_micros), Some(output_tokens)) = (
        usage.ttft_micros,
        usage.generation_micros,
        usage.output_tokens,
    ) {
        entry.timing_sample_count = entry.timing_sample_count.saturating_add(1);
        entry.total_ttft_micros = entry.total_ttft_micros.saturating_add(ttft_micros);
        entry.total_generation_micros = entry
            .total_generation_micros
            .saturating_add(generation_micros);
        entry.timed_output_tokens = entry.timed_output_tokens.saturating_add(output_tokens);
        if generation_micros > 0 {
            entry.total_output_tps += output_tokens as f64 * 1_000_000.0 / generation_micros as f64;
            entry.tps_sample_count = entry.tps_sample_count.saturating_add(1);
        }
    }
}

fn add_aggregated_usage(entry: &mut ProviderTokenUsage, usage: &ProviderTokenUsage) {
    entry.provider_id = usage.provider_id.clone();
    entry.model_id = usage.model_id.clone();
    entry.request_count = entry.request_count.saturating_add(usage.request_count);
    entry.input_tokens = entry.input_tokens.saturating_add(usage.input_tokens);
    entry.cache_read_tokens = entry
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    entry.cache_creation_tokens = entry
        .cache_creation_tokens
        .saturating_add(usage.cache_creation_tokens);
    entry.output_tokens = entry.output_tokens.saturating_add(usage.output_tokens);
    entry.observed_cache_read_tokens = entry
        .observed_cache_read_tokens
        .saturating_add(usage.observed_cache_read_tokens);
    entry.observed_uncached_input_tokens = entry
        .observed_uncached_input_tokens
        .saturating_add(usage.observed_uncached_input_tokens);
    entry.timing_sample_count = entry
        .timing_sample_count
        .saturating_add(usage.timing_sample_count);
    entry.total_ttft_micros = entry
        .total_ttft_micros
        .saturating_add(usage.total_ttft_micros);
    entry.total_generation_micros = entry
        .total_generation_micros
        .saturating_add(usage.total_generation_micros);
    entry.timed_output_tokens = entry
        .timed_output_tokens
        .saturating_add(usage.timed_output_tokens);
    entry.total_output_tps += usage.total_output_tps;
    entry.tps_sample_count = entry
        .tps_sample_count
        .saturating_add(usage.tps_sample_count);
}

fn finalize_usage(mut entries: Vec<ProviderTokenUsage>) -> Vec<ProviderTokenUsage> {
    for entry in &mut entries {
        let observed = entry
            .observed_cache_read_tokens
            .saturating_add(entry.observed_uncached_input_tokens);
        entry.cache_hit_percent = (observed > 0)
            .then(|| entry.observed_cache_read_tokens as f64 / observed as f64 * 100.0);
        entry.average_ttft_ms = (entry.timing_sample_count > 0)
            .then(|| entry.total_ttft_micros as f64 / entry.timing_sample_count as f64 / 1_000.0);
        entry.output_tps = (entry.total_generation_micros > 0).then(|| {
            entry.timed_output_tokens as f64 * 1_000_000.0 / entry.total_generation_micros as f64
        });
    }
    entries.sort_by(|left, right| {
        left.provider_id
            .cmp(&right.provider_id)
            .then_with(|| left.model_id.cmp(&right.model_id))
    });
    entries
}

/// Provider cache counters observed on a streamed response. Each protocol names
/// them differently, so they are normalised to uncached input plus cache reads.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct UpstreamCacheUsage {
    pub(crate) input_tokens: Option<u64>,
    pub(crate) cache_read_tokens: Option<u64>,
    pub(crate) cache_creation_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) ttft_micros: Option<u64>,
    pub(crate) generation_micros: Option<u64>,
}

impl UpstreamCacheUsage {
    pub(crate) fn absorb(&mut self, usage: &Value) {
        // Anthropic Messages reports uncached input directly; DeepSeek and other
        // Chat Completions upstreams split it into hit and miss counters, and
        // OpenAI Responses nests the hit inside `input_tokens_details`.
        let nested_cached = |parent: &str| {
            usage
                .get(parent)
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
        };
        let field = |name: &str| usage.get(name).and_then(Value::as_u64);
        let cache_read = field("cache_read_input_tokens")
            .or_else(|| field("prompt_cache_hit_tokens"))
            .or_else(|| nested_cached("prompt_tokens_details"))
            .or_else(|| nested_cached("input_tokens_details"));
        if let Some(cache_read) = cache_read {
            self.cache_read_tokens = Some(cache_read);
        }
        // `prompt_tokens` counts the whole prompt including cache reads, so the
        // uncached part has to be derived to stay comparable across protocols.
        let input = field("prompt_cache_miss_tokens")
            .or_else(|| {
                field("prompt_tokens")
                    .map(|prompt| prompt.saturating_sub(self.cache_read_tokens.unwrap_or(0)))
            })
            .or_else(|| {
                field("input_tokens").map(|input| {
                    if usage.get("input_tokens_details").is_some() {
                        input.saturating_sub(self.cache_read_tokens.unwrap_or(0))
                    } else {
                        input
                    }
                })
            });
        if let Some(input) = input {
            self.input_tokens = Some(input);
        }
        if let Some(created) = field("cache_creation_input_tokens") {
            self.cache_creation_tokens = Some(created);
        }
        if let Some(output) = field("output_tokens").or_else(|| field("completion_tokens")) {
            self.output_tokens = Some(output);
        }
    }

    /// True once the prompt accounting is actually populated. Anthropic sends a
    /// zeroed `usage` on `message_start` and the real counts on `message_delta`,
    /// so reporting on the first `usage` seen would score every turn as a miss.
    pub(crate) fn observed(&self) -> bool {
        [self.input_tokens, self.cache_read_tokens]
            .into_iter()
            .flatten()
            .any(|value| value > 0)
    }
}
