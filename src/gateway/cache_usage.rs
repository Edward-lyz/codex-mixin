//! Provider usage accounting and persistence for prompt-cache diagnostics.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

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
    #[serde(skip)]
    pub(crate) observed_cache_read_tokens: u64,
    #[serde(skip)]
    pub(crate) observed_uncached_input_tokens: u64,
}

#[derive(Debug, Default)]
struct TokenUsageState {
    entries: HashMap<(String, String), ProviderTokenUsage>,
}

fn load_persisted_usage(
    path: &Path,
) -> anyhow::Result<HashMap<(String, String), ProviderTokenUsage>> {
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
            PRIMARY KEY (provider_id, model_id)
        )",
    )?;
    let mut statement = connection.prepare(
        "SELECT provider_id, model_id, request_count, input_tokens, cache_read_tokens,
                cache_creation_tokens, output_tokens, observed_cache_read_tokens,
                observed_uncached_input_tokens
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
                observed_cache_read_tokens: row.get(7)?,
                observed_uncached_input_tokens: row.get(8)?,
            },
        ))
    })?;
    Ok(rows.collect::<Result<HashMap<_, _>, _>>()?)
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
    let connection = Connection::open(path)?;
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
            PRIMARY KEY (provider_id, model_id)
         )",
    )?;
    connection.execute(
        "INSERT INTO token_usage (
            provider_id, model_id, request_count, input_tokens, cache_read_tokens,
            cache_creation_tokens, output_tokens, observed_cache_read_tokens,
            observed_uncached_input_tokens
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(provider_id, model_id) DO UPDATE SET
            request_count = token_usage.request_count + excluded.request_count,
            input_tokens = token_usage.input_tokens + excluded.input_tokens,
            cache_read_tokens = token_usage.cache_read_tokens + excluded.cache_read_tokens,
            cache_creation_tokens = token_usage.cache_creation_tokens + excluded.cache_creation_tokens,
            output_tokens = token_usage.output_tokens + excluded.output_tokens,
            observed_cache_read_tokens = token_usage.observed_cache_read_tokens + excluded.observed_cache_read_tokens,
            observed_uncached_input_tokens = token_usage.observed_uncached_input_tokens + excluded.observed_uncached_input_tokens",
        params![
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
        ],
    )?;
    Ok(())
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
        let entries = match path.as_deref() {
            Some(path) if path.exists() => load_persisted_usage(path)?,
            _ => HashMap::new(),
        };
        Ok(Self {
            state: Mutex::new(TokenUsageState { entries }),
            persist_path: path,
        })
    }

    pub(crate) fn record(&self, provider_id: &str, model_id: &str, usage: &UpstreamCacheUsage) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = state
            .entries
            .entry((provider_id.to_owned(), model_id.to_owned()))
            .or_default();
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
        let mut entries = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for entry in &mut entries {
            let observed = entry
                .observed_cache_read_tokens
                .saturating_add(entry.observed_uncached_input_tokens);
            entry.cache_hit_percent = (observed > 0)
                .then(|| entry.observed_cache_read_tokens as f64 / observed as f64 * 100.0);
        }
        entries.sort_by(|left, right| {
            left.provider_id
                .cmp(&right.provider_id)
                .then_with(|| left.model_id.cmp(&right.model_id))
        });
        entries
    }
}

/// Provider cache counters observed on a streamed response. Each protocol names
/// them differently, so they are normalised to uncached input plus cache reads.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct UpstreamCacheUsage {
    pub(crate) input_tokens: Option<u64>,
    pub(crate) cache_read_tokens: Option<u64>,
    pub(crate) cache_creation_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
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
