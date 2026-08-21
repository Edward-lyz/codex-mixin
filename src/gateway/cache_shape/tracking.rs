use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::Value;

use crate::sse::SseDecoder;
use crate::upstream::UpstreamRouting;

use super::super::cache_usage::{ProviderTokenUsage, TokenUsageAggregator, UpstreamCacheUsage};
use super::shape::{CacheShape, PrefixChanges, PrefixReport, PrefixState};

/// Sessions retained for prefix diagnostics. Codex drives one session per
/// conversation, so this only has to cover concurrently active threads.
pub(super) const TRACKED_SESSIONS: usize = 64;

/// Shapes retained per session. Fusion panels, judges and concurrent subagents
/// can share one session id while sending unrelated prompts, so a single slot
/// would report their interleaving as prefix loss.
pub(super) const TRACKED_SHAPES_PER_SESSION: usize = 4;

pub(super) struct TrackedSession {
    pub(super) seq: u64,
    /// Recently sent shapes, oldest first.
    pub(super) shapes: Vec<CacheShape>,
}

#[derive(Default)]
pub(super) struct TrackedSessions {
    pub(super) next_seq: u64,
    pub(super) entries: HashMap<String, TrackedSession>,
}

/// Recent provider-visible cache shapes per session, used to detect prefix loss
/// across turns on both the HTTP and WebSocket paths.
#[derive(Default)]
pub(crate) struct CacheShapeTracker {
    pub(super) sessions: Mutex<TrackedSessions>,
    pub(super) usage: Arc<TokenUsageAggregator>,
}

impl CacheShapeTracker {
    pub(crate) fn with_usage(usage: TokenUsageAggregator) -> Self {
        Self {
            sessions: Mutex::new(TrackedSessions::default()),
            usage: Arc::new(usage),
        }
    }

    pub(crate) fn usage(&self) -> Arc<TokenUsageAggregator> {
        self.usage.clone()
    }

    pub(crate) fn usage_snapshot(&self) -> Vec<ProviderTokenUsage> {
        self.usage.snapshot()
    }

    pub(crate) fn usage_snapshot_for_days(
        &self,
        days: u64,
    ) -> anyhow::Result<Vec<ProviderTokenUsage>> {
        self.usage.snapshot_for_days(days)
    }

    pub(crate) fn record(&self, session_key: &str, shape: CacheShape) -> PrefixReport {
        let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        let seq = sessions.next_seq;
        sessions.next_seq += 1;
        let tracked = sessions
            .entries
            .entry(session_key.to_owned())
            .or_insert_with(|| TrackedSession {
                seq,
                shapes: Vec::new(),
            });
        tracked.seq = seq;
        // Compare against every remembered lineage and keep the best match, so an
        // unrelated prompt sharing the session cannot look like prefix loss.
        // Matching tools and system come first: a subagent or second window in the
        // same session carries its own tool set, and comparing across those two
        // would report a tool drift that never happened.
        let report = tracked
            .shapes
            .iter()
            .map(|earlier| shape.compare(earlier))
            .max_by_key(|report| {
                (
                    !report.changes.tools,
                    !report.changes.system,
                    report.reused_turns,
                    !report.state.is_cache_loss(),
                )
            })
            .unwrap_or(PrefixReport {
                state: PrefixState::ColdStart,
                changes: PrefixChanges::default(),
                reused_turns: 0,
                reused_bytes: 0,
                stable_prefix_bytes: 0,
                total_bytes: shape.total_bytes(),
                message_prefix_turns: 0,
                previous_turns: 0,
                total_turns: shape.turns.len(),
                system_prefix_blocks: 0,
                previous_system_blocks: 0,
                system_blocks: shape.system.len(),
                tools_count: shape.tools_count(),
                previous_tools_count: 0,
            });
        tracked.shapes.retain(|earlier| earlier != &shape);
        tracked.shapes.push(shape);
        if tracked.shapes.len() > TRACKED_SHAPES_PER_SESSION {
            tracked.shapes.remove(0);
        }
        if sessions.entries.len() > TRACKED_SESSIONS {
            let stale = sessions
                .entries
                .iter()
                .min_by_key(|(_, tracked)| tracked.seq)
                .map(|(key, _)| key.clone());
            if let Some(stale) = stale {
                sessions.entries.remove(&stale);
            }
        }
        report
    }
}

/// Records the request shape for this session and logs where the provider
/// prompt cache stands. Cache loss is a warning because it is actionable;
/// append-only turns stay at debug.
pub(crate) fn record_provider_prefix(
    tracker: &CacheShapeTracker,
    provider_id: &str,
    catalog_slug: &str,
    upstream_model_id: &str,
    routing: Option<&UpstreamRouting>,
    shape: CacheShape,
) -> Option<PrefixObservation> {
    let Some(routing) = routing else {
        tracing::debug!(
            provider_id,
            catalog_slug,
            upstream_model_id,
            protocol = shape.protocol,
            "provider request has no session key, so prefix cache tracking is unavailable"
        );
        return None;
    };
    // Fusion panels and judges share one Codex session while sending different
    // prompts, so the tracked key has to include the upstream model.
    let session_key = format!(
        "{}|{provider_id}|{catalog_slug}|{upstream_model_id}",
        routing.session_id
    );
    let protocol = shape.protocol;
    let shape_hash = shape.shape_hash();
    let system_hash = shape.system_hash();
    let tools_hash = shape.tools.hash;
    let report = tracker.record(&session_key, shape);
    let changed_regions = report.changes.as_list();
    if report.state.is_cache_loss() {
        tracing::warn!(
            provider_id,
            catalog_slug,
            upstream_model_id,
            protocol,
            prefix_state = report.state.as_str(),
            changed_regions = changed_regions.as_str(),
            reused_turns = report.reused_turns,
            reused_bytes = report.reused_bytes,
            message_prefix_turns = report.message_prefix_turns,
            previous_turns = report.previous_turns,
            total_turns = report.total_turns,
            system_prefix_blocks = report.system_prefix_blocks,
            previous_system_blocks = report.previous_system_blocks,
            system_blocks = report.system_blocks,
            tools_count = report.tools_count,
            previous_tools_count = report.previous_tools_count,
            shape_hash,
            system_hash,
            tools_hash,
            "provider prompt prefix cache was invalidated"
        );
    } else {
        tracing::debug!(
            provider_id,
            catalog_slug,
            upstream_model_id,
            protocol,
            prefix_state = report.state.as_str(),
            changed_regions = changed_regions.as_str(),
            reused_turns = report.reused_turns,
            reused_bytes = report.reused_bytes,
            message_prefix_turns = report.message_prefix_turns,
            previous_turns = report.previous_turns,
            total_turns = report.total_turns,
            system_prefix_blocks = report.system_prefix_blocks,
            previous_system_blocks = report.previous_system_blocks,
            system_blocks = report.system_blocks,
            shape_hash,
            system_hash,
            tools_hash,
            "provider prompt prefix cache shape"
        );
    }
    Some(PrefixObservation {
        provider_id: provider_id.to_owned(),
        catalog_slug: catalog_slug.to_owned(),
        upstream_model_id: upstream_model_id.to_owned(),
        protocol,
        state: report.state,
        changed_regions,
        stable_prefix_bytes: report.stable_prefix_bytes,
        total_bytes: report.total_bytes,
        reused_turns: report.reused_turns,
        total_turns: report.total_turns,
        usage: tracker.usage(),
    })
}

/// Prompts below this size are too small for provider caches to act on, so a
/// miss there says nothing about the request shape.
pub(super) const MIN_PROMPT_TOKENS_FOR_VERDICT: u64 = 8_192;

/// Fraction of the reusable prefix a provider has to serve from cache before the
/// turn counts as healthy. Well below 1.0 because the byte ratio and the tokenizer
/// never line up exactly, and because providers round cache hits down to a block.
pub(super) const MIN_SERVED_FRACTION_OF_REUSABLE: f64 = 0.5;

/// What this gateway sent for one request, kept so the provider usage counters
/// that arrive later can be judged against the prefix we know we preserved.
#[derive(Clone, Debug)]
pub(crate) struct PrefixObservation {
    pub(super) provider_id: String,
    pub(super) catalog_slug: String,
    pub(super) upstream_model_id: String,
    pub(super) protocol: &'static str,
    pub(super) state: PrefixState,
    pub(super) changed_regions: String,
    pub(super) stable_prefix_bytes: usize,
    pub(super) total_bytes: usize,
    pub(super) reused_turns: usize,
    pub(super) total_turns: usize,
    pub(super) usage: Arc<TokenUsageAggregator>,
}

impl PrefixObservation {
    /// Tokens the provider could have served from cache on this request.
    ///
    /// Derived as the byte share of the stable prefix applied to the token count
    /// the provider itself reported, so the bytes-per-token rate cancels out. A
    /// fixed share of the whole prompt cannot work here: a turn that appends a
    /// large tool result legitimately reuses only half its prompt, while a turn
    /// that appends one line should reuse nearly all of it.
    pub(super) fn reusable_tokens(&self, prompt_tokens: u64) -> u64 {
        if self.total_bytes == 0 {
            return 0;
        }
        let share = self.stable_prefix_bytes as f64 / self.total_bytes as f64;
        (prompt_tokens as f64 * share) as u64
    }

    /// True when this gateway kept a substantial prefix byte-identical and the
    /// provider still recomputed most of it.
    ///
    /// The verdict uses the provider's own token counters rather than a
    /// byte-to-token estimate: bytes per token swings by more than 3x between
    /// ASCII code and CJK prose, which is enough to score a 98% cache hit as a
    /// miss.
    pub(super) fn discarded_by_provider(&self, usage: &UpstreamCacheUsage) -> bool {
        // A provider that never reports cache counters at all cannot be judged:
        // Baidu OneAPI's Opus route caches but omits the fields, so treating a
        // missing counter as a miss would report every turn as a finding.
        let (Some(cache_read_tokens), Some(uncached_tokens)) =
            (usage.cache_read_tokens, usage.input_tokens)
        else {
            return false;
        };
        let prompt_tokens = cache_read_tokens.saturating_add(uncached_tokens);
        // A cold start has no prefix to reuse, so an uncached prompt is expected.
        if self.state.is_cache_loss()
            || self.stable_prefix_bytes == 0
            || prompt_tokens < MIN_PROMPT_TOKENS_FOR_VERDICT
        {
            return false;
        }
        let reusable = self.reusable_tokens(prompt_tokens);
        if reusable < MIN_PROMPT_TOKENS_FOR_VERDICT {
            return false;
        }
        (cache_read_tokens as f64) < reusable as f64 * MIN_SERVED_FRACTION_OF_REUSABLE
    }

    /// Logs the provider cache counters next to the prefix this gateway
    /// preserved. A provider that recomputes a prefix we kept byte-identical is
    /// the only case that warrants a warning, because nothing on this side can
    /// fix it and it otherwise looks like a gateway bug.
    pub(crate) fn report_upstream_cache(&self, usage: &UpstreamCacheUsage) {
        self.usage
            .record(&self.provider_id, &self.upstream_model_id, usage);
        let cache_read_tokens = usage.cache_read_tokens.unwrap_or(0);
        let uncached_input_tokens = usage.input_tokens.unwrap_or(0);
        let prompt_tokens = cache_read_tokens.saturating_add(uncached_input_tokens);
        let reusable_tokens = self.reusable_tokens(prompt_tokens);
        if self.discarded_by_provider(usage) {
            tracing::warn!(
                provider_id = self.provider_id,
                catalog_slug = self.catalog_slug,
                upstream_model_id = self.upstream_model_id,
                protocol = self.protocol,
                prefix_state = self.state.as_str(),
                reused_turns = self.reused_turns,
                total_turns = self.total_turns,
                stable_prefix_bytes = self.stable_prefix_bytes,
                reusable_tokens,
                prompt_tokens,
                cache_read_tokens,
                uncached_input_tokens,
                cache_creation_tokens = usage.cache_creation_tokens.unwrap_or(0),
                "provider recomputed a prompt prefix this gateway kept byte-identical"
            );
            return;
        }
        tracing::debug!(
            provider_id = self.provider_id,
            catalog_slug = self.catalog_slug,
            upstream_model_id = self.upstream_model_id,
            protocol = self.protocol,
            prefix_state = self.state.as_str(),
            changed_regions = self.changed_regions.as_str(),
            reused_turns = self.reused_turns,
            total_turns = self.total_turns,
            stable_prefix_bytes = self.stable_prefix_bytes,
            reusable_tokens,
            prompt_tokens,
            cache_read_tokens,
            uncached_input_tokens,
            cache_creation_tokens = usage.cache_creation_tokens.unwrap_or(0),
            "provider prompt cache usage"
        );
    }
}

/// Holds the counters until the upstream stream is done with them. Anthropic
/// upstreams send a partial `usage` frame before the final one, so a verdict
/// taken on the first non-zero counter scores a cache hit as a miss. Reporting
/// from `Drop` covers both endings that occur in practice: the stream running to
/// completion, and a downstream mapper stopping once it sees the terminal event.
pub(crate) struct UpstreamCacheObserver {
    observation: PrefixObservation,
    usage: UpstreamCacheUsage,
}

impl UpstreamCacheObserver {
    pub(crate) fn new(observation: PrefixObservation) -> Self {
        Self {
            observation,
            usage: UpstreamCacheUsage::default(),
        }
    }

    pub(crate) fn observe_value(&mut self, value: &Value) {
        for path in [
            value,
            value.get("message").unwrap_or(&Value::Null),
            value.get("response").unwrap_or(&Value::Null),
        ] {
            if let Some(usage) = path.get("usage") {
                self.usage.absorb(usage);
            }
        }
    }
}

impl Drop for UpstreamCacheObserver {
    fn drop(&mut self) {
        if self.usage.observed() {
            self.observation.report_upstream_cache(&self.usage);
        }
    }
}

/// Passes an upstream SSE byte stream through untouched while collecting the
/// prompt cache counters, then reports them against the recorded prefix. Reading
/// the counters here keeps the downstream event mappers unaware of diagnostics
/// and works for every protocol this gateway speaks upstream.
pub(crate) fn observe_upstream_cache_usage<S>(
    upstream: S,
    observation: Option<PrefixObservation>,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    async_stream::stream! {
        let mut decoder = SseDecoder::default();
        let mut report = observation.map(UpstreamCacheObserver::new);
        tokio::pin!(upstream);
        while let Some(chunk) = upstream.next().await {
            if let Ok(bytes) = &chunk
                && let Some(report) = report.as_mut()
            {
                for event in decoder.push(bytes) {
                    let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
                        continue;
                    };
                    report.observe_value(&data);
                }
            }
            yield chunk;
        }
    }
}
