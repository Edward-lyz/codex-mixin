//! Prompt-prefix cache contract for provider-visible requests.
//!
//! Providers cache on the rendered prompt prefix: system prompt, tool
//! definitions, then the message sequence in order. A session keeps that cache
//! only while those regions stay byte-identical and every new turn is appended
//! at the tail. This module derives the shape from the exact bytes each
//! protocol serializes upstream and reports where a session lost its prefix, so
//! a cache miss has a concrete cause instead of a guess.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use serde_json::Value;

use crate::anthropic::MessageRequest;
use crate::sse::SseDecoder;
use crate::upstream::UpstreamRouting;

pub(crate) const ANTHROPIC_MESSAGES: &str = "anthropic_messages";
pub(crate) const OPENAI_CHAT: &str = "openai_chat";
pub(crate) const OPENAI_RESPONSES: &str = "openai_responses";

/// Sessions retained for prefix diagnostics. Codex drives one session per
/// conversation, so this only has to cover concurrently active threads.
const TRACKED_SESSIONS: usize = 64;

/// Shapes retained per session. Fusion panels, judges and concurrent subagents
/// can share one session id while sending unrelated prompts, so a single slot
/// would report their interleaving as prefix loss.
const TRACKED_SHAPES_PER_SESSION: usize = 4;

/// History this long, replaced wholesale by a much shorter one, is compaction
/// rather than prompt drift. Below this the two are indistinguishable from a
/// short session that simply restarted.
const MIN_TURNS_FOR_REPLACED_HISTORY: usize = 8;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Provider-level token and prompt cache counters observed on upstream
/// responses, kept compact so the menu can visualize usage without retaining
/// request bodies or history.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct ProviderTokenUsage {
    pub(crate) provider_id: String,
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
    entries: HashMap<String, ProviderTokenUsage>,
}

/// In-memory aggregate of the counters seen on streamed provider responses.
#[derive(Debug, Default)]
pub(crate) struct TokenUsageAggregator {
    state: Mutex<TokenUsageState>,
}

impl TokenUsageAggregator {
    fn record(&self, provider_id: &str, usage: &UpstreamCacheUsage) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = state.entries.entry(provider_id.to_owned()).or_default();
        entry.provider_id = provider_id.to_owned();
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
        entries.sort_by(|left, right| left.provider_id.cmp(&right.provider_id));
        entries
    }
}

/// Streaming FNV-1a sink. Serializing straight into the hasher keeps the digest
/// over the same bytes `reqwest` sends without buffering a second copy of the
/// request.
struct ShapeHasher {
    hash: u64,
    bytes: usize,
}

impl ShapeHasher {
    fn new() -> Self {
        Self {
            hash: FNV_OFFSET,
            bytes: 0,
        }
    }
}

impl Write for ShapeHasher {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for &byte in buf {
            self.hash ^= u64::from(byte);
            self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
        self.bytes += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Digest of one cache-relevant region, taken over its upstream JSON bytes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RegionDigest {
    hash: u64,
    bytes: usize,
}

impl RegionDigest {
    /// Region the protocol does not send at all. Distinct from a region that is
    /// present but empty, because those affect the prompt differently.
    const ABSENT: Self = Self { hash: 0, bytes: 0 };

    fn of<T: Serialize + ?Sized>(value: &T) -> Self {
        let mut hasher = ShapeHasher::new();
        if serde_json::to_writer(&mut hasher, value).is_err() {
            return Self::ABSENT;
        }
        Self {
            hash: hasher.hash,
            bytes: hasher.bytes,
        }
    }

    fn of_optional(value: Option<&Value>) -> Self {
        value.map_or(Self::ABSENT, Self::of)
    }
}

/// Cache-relevant regions of a single provider-visible request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CacheShape {
    protocol: &'static str,
    model: String,
    /// System prompt carrier, one digest per block: Anthropic `system` blocks,
    /// Responses `instructions`, or the leading `system` chat message.
    ///
    /// Kept per block because Codex appends a developer message such as
    /// `<workspace_context>` every turn. A single digest would only say the
    /// system prompt moved, not which block moved it.
    system: Vec<RegionDigest>,
    /// Tool configuration, covering both the definitions and `tool_choice`,
    /// because either one shifts the cached tool preamble.
    tools: RegionDigest,
    /// How many tool definitions the digest covers.
    tools_count: usize,
    /// Reasoning configuration, which providers fold into the cached prefix.
    config: RegionDigest,
    /// One digest per message, in wire order.
    turns: Vec<RegionDigest>,
}

impl CacheShape {
    pub(crate) fn from_anthropic(request: &MessageRequest) -> Self {
        Self {
            protocol: ANTHROPIC_MESSAGES,
            model: request.model.clone(),
            system: request.system.as_ref().map_or_else(Vec::new, |blocks| {
                blocks.iter().map(RegionDigest::of).collect()
            }),
            tools: RegionDigest::of(&(&request.tools, &request.tool_choice)),
            tools_count: request.tools.len(),
            config: request
                .thinking
                .as_ref()
                .map_or(RegionDigest::ABSENT, RegionDigest::of),
            turns: request.messages.iter().map(RegionDigest::of).collect(),
        }
    }

    pub(crate) fn from_openai_chat(request: &Value) -> Self {
        let messages = request.get("messages").and_then(Value::as_array);
        Self {
            protocol: OPENAI_CHAT,
            model: request_model(request),
            system: messages
                .and_then(|messages| messages.first())
                .filter(|message| message.get("role").and_then(Value::as_str) == Some("system"))
                .map(RegionDigest::of)
                .into_iter()
                .collect(),
            tools: RegionDigest::of(&(request.get("tools"), request.get("tool_choice"))),
            tools_count: value_len(request.get("tools")),
            config: RegionDigest::ABSENT,
            turns: turn_digests(messages),
        }
    }

    pub(crate) fn from_openai_responses(request: &Value) -> Self {
        Self {
            protocol: OPENAI_RESPONSES,
            model: request_model(request),
            system: request
                .get("instructions")
                .map(RegionDigest::of)
                .into_iter()
                .collect(),
            tools: RegionDigest::of(&(request.get("tools"), request.get("tool_choice"))),
            tools_count: value_len(request.get("tools")),
            config: RegionDigest::of_optional(request.get("reasoning")),
            turns: turn_digests(request.get("input").and_then(Value::as_array)),
        }
    }

    /// Stable identity of the whole cache shape, folded from the region digests
    /// instead of re-serializing the request.
    fn shape_hash(&self) -> u64 {
        let mut hasher = ShapeHasher::new();
        let _ = write!(
            hasher,
            "{}|{}|{}|{}",
            self.protocol, self.model, self.tools.hash, self.config.hash
        );
        for block in &self.system {
            let _ = write!(hasher, "|s{}", block.hash);
        }
        for turn in &self.turns {
            let _ = write!(hasher, "|{}", turn.hash);
        }
        hasher.hash
    }

    fn system_hash(&self) -> u64 {
        let mut hasher = ShapeHasher::new();
        for block in &self.system {
            let _ = write!(hasher, "|{}", block.hash);
        }
        hasher.hash
    }

    /// Total bytes of every cache-relevant region. Used with
    /// `stable_prefix_bytes` to express prefix reuse as a ratio, which is the
    /// only way to compare it against provider token counters without guessing
    /// a bytes-per-token rate.
    fn total_bytes(&self) -> usize {
        self.system.iter().map(|block| block.bytes).sum::<usize>()
            + self.tools.bytes
            + self.config.bytes
            + self.turns.iter().map(|turn| turn.bytes).sum::<usize>()
    }

    /// Number of tool definitions is not recoverable from a digest, so it is
    /// tracked separately: a tool set that changes hash while keeping its size
    /// points at ordering or field drift, which is fixable here, while a changed
    /// size means the client really added or removed tools.
    fn tools_count(&self) -> usize {
        self.tools_count
    }

    /// Compares every region instead of returning on the first difference.
    ///
    /// A changed system prompt already costs the whole prefix, but stopping
    /// there also hides an independent problem in the message sequence.
    fn compare(&self, previous: &Self) -> PrefixReport {
        let changes = PrefixChanges {
            protocol: previous.protocol != self.protocol,
            model: previous.model != self.model,
            system: previous.system != self.system,
            tools: previous.tools != self.tools,
            config: previous.config != self.config,
        };
        let message_prefix_turns = common_prefix_len(&self.turns, &previous.turns);
        let turn_state = if message_prefix_turns == previous.turns.len() {
            None
        } else if message_prefix_turns == self.turns.len() {
            Some(PrefixState::HistoryTruncated)
        } else if message_prefix_turns + 1 == previous.turns.len() {
            // Converters legitimately merge a freshly appended tool result into
            // the previous trailing message, so only the final earlier turn
            // moves. Every turn before it still caches.
            Some(PrefixState::TailRewritten)
        } else {
            Some(PrefixState::TurnRewritten)
        };
        // A region change invalidates the prompt from its first token, so no
        // message prefix survives however clean the message sequence is.
        let reused_turns = if changes.any() {
            0
        } else {
            message_prefix_turns
        };
        // Compaction swaps the whole transcript for a summary and rewrites the
        // system prompt on the way, so the region change is a symptom. Naming the
        // replaced history as the headline keeps the cause readable instead of
        // reporting it as instructions drifting.
        let history_replaced = message_prefix_turns == 0
            && previous.turns.len() >= MIN_TURNS_FOR_REPLACED_HISTORY
            && self.turns.len().saturating_mul(4) <= previous.turns.len();
        PrefixReport {
            state: if history_replaced {
                PrefixState::HistoryTruncated
            } else {
                changes
                    .state()
                    .or(turn_state)
                    .unwrap_or(PrefixState::AppendOnly)
            },
            changes,
            reused_turns,
            reused_bytes: self.turns[..reused_turns]
                .iter()
                .map(|turn| turn.bytes)
                .sum(),
            stable_prefix_bytes: self.stable_prefix_bytes(previous, &changes, reused_turns),
            total_bytes: self.total_bytes(),
            message_prefix_turns,
            previous_turns: previous.turns.len(),
            total_turns: self.turns.len(),
            system_prefix_blocks: common_prefix_len(&self.system, &previous.system),
            previous_system_blocks: previous.system.len(),
            system_blocks: self.system.len(),
            tools_count: self.tools_count,
            previous_tools_count: previous.tools_count,
        }
    }

    /// Bytes a provider cache could have reused: the surviving system blocks,
    /// the tool definitions when they did not move, and every replayed turn.
    /// This is the baseline a provider cache hit is judged against, so it has to
    /// span every region ahead of the new tail, not only the message list.
    fn stable_prefix_bytes(
        &self,
        previous: &Self,
        changes: &PrefixChanges,
        reused_turns: usize,
    ) -> usize {
        if changes.any() {
            return 0;
        }
        let system = self.system[..common_prefix_len(&self.system, &previous.system)]
            .iter()
            .map(|block| block.bytes)
            .sum::<usize>();
        let turns = self.turns[..reused_turns]
            .iter()
            .map(|turn| turn.bytes)
            .sum::<usize>();
        system + self.tools.bytes + turns
    }
}

fn common_prefix_len(current: &[RegionDigest], earlier: &[RegionDigest]) -> usize {
    current
        .iter()
        .zip(earlier)
        .take_while(|(current, earlier)| current == earlier)
        .count()
}

fn request_model(request: &Value) -> String {
    request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn turn_digests(turns: Option<&Vec<Value>>) -> Vec<RegionDigest> {
    turns
        .map(|turns| turns.iter().map(RegionDigest::of).collect())
        .unwrap_or_default()
}

fn value_len(value: Option<&Value>) -> usize {
    value.and_then(Value::as_array).map_or(0, Vec::len)
}

/// How much of the previous provider prompt this request can still reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PrefixReport {
    pub(crate) state: PrefixState,
    /// Every region that changed, not only the one that decided `state`.
    pub(crate) changes: PrefixChanges,
    /// Turns the provider can actually reuse. Zero whenever a region changed.
    pub(crate) reused_turns: usize,
    pub(crate) reused_bytes: usize,
    /// Bytes of prompt ahead of the new tail that stayed byte-identical, across
    /// system, tools and replayed turns.
    pub(crate) stable_prefix_bytes: usize,
    /// Bytes of every cache-relevant region in this request.
    pub(crate) total_bytes: usize,
    /// Length of the byte-identical message prefix, regardless of regions. This
    /// separates "the history is clean but the system prompt moved" from "the
    /// history itself was rewritten".
    pub(crate) message_prefix_turns: usize,
    pub(crate) previous_turns: usize,
    pub(crate) total_turns: usize,
    /// Number of leading system blocks that survived byte-identical.
    pub(crate) system_prefix_blocks: usize,
    pub(crate) previous_system_blocks: usize,
    pub(crate) system_blocks: usize,
    pub(crate) tools_count: usize,
    pub(crate) previous_tools_count: usize,
}

/// Which cache-relevant regions differ from the previous request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PrefixChanges {
    pub(crate) protocol: bool,
    pub(crate) model: bool,
    pub(crate) system: bool,
    pub(crate) tools: bool,
    pub(crate) config: bool,
}

impl PrefixChanges {
    fn any(self) -> bool {
        self.protocol || self.model || self.system || self.tools || self.config
    }

    /// The most fundamental change, used as the headline state.
    fn state(self) -> Option<PrefixState> {
        if self.protocol {
            Some(PrefixState::ProtocolChanged)
        } else if self.model {
            Some(PrefixState::ModelChanged)
        } else if self.system {
            Some(PrefixState::SystemChanged)
        } else if self.tools {
            Some(PrefixState::ToolsChanged)
        } else if self.config {
            Some(PrefixState::ConfigChanged)
        } else {
            None
        }
    }

    /// Comma-separated region names for logging, empty when nothing changed.
    pub(crate) fn as_list(self) -> String {
        let mut regions = Vec::new();
        for (changed, name) in [
            (self.protocol, "protocol"),
            (self.model, "model"),
            (self.system, "system"),
            (self.tools, "tools"),
            (self.config, "config"),
        ] {
            if changed {
                regions.push(name);
            }
        }
        regions.join(",")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrefixState {
    /// No earlier request recorded for this session.
    ColdStart,
    /// Every earlier turn survived byte-identical and new turns were appended.
    AppendOnly,
    /// Only the final earlier turn changed; the prefix before it is intact.
    TailRewritten,
    ModelChanged,
    ProtocolChanged,
    SystemChanged,
    ToolsChanged,
    ConfigChanged,
    /// An earlier turn was rewritten, so the provider recomputes from there.
    TurnRewritten,
    /// History shrank, which is what compaction looks like from upstream.
    HistoryTruncated,
}

impl PrefixState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ColdStart => "cold_start",
            Self::AppendOnly => "append_only",
            Self::TailRewritten => "tail_rewritten",
            Self::ModelChanged => "model_changed",
            Self::ProtocolChanged => "protocol_changed",
            Self::SystemChanged => "system_changed",
            Self::ToolsChanged => "tools_changed",
            Self::ConfigChanged => "config_changed",
            Self::TurnRewritten => "turn_rewritten",
            Self::HistoryTruncated => "history_truncated",
        }
    }

    /// True when the provider has to recompute prompt prefix it already had.
    pub(crate) fn is_cache_loss(self) -> bool {
        !matches!(
            self,
            Self::ColdStart | Self::AppendOnly | Self::TailRewritten
        )
    }
}

struct TrackedSession {
    seq: u64,
    /// Recently sent shapes, oldest first.
    shapes: Vec<CacheShape>,
}

#[derive(Default)]
struct TrackedSessions {
    next_seq: u64,
    entries: HashMap<String, TrackedSession>,
}

/// Recent provider-visible cache shapes per session, used to detect prefix loss
/// across turns on both the HTTP and WebSocket paths.
#[derive(Default)]
pub(crate) struct CacheShapeTracker {
    sessions: Mutex<TrackedSessions>,
    usage: Arc<TokenUsageAggregator>,
}

impl CacheShapeTracker {
    pub(crate) fn usage(&self) -> Arc<TokenUsageAggregator> {
        self.usage.clone()
    }

    pub(crate) fn usage_snapshot(&self) -> Vec<ProviderTokenUsage> {
        self.usage.snapshot()
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
const MIN_PROMPT_TOKENS_FOR_VERDICT: u64 = 8_192;

/// Fraction of the reusable prefix a provider has to serve from cache before the
/// turn counts as healthy. Well below 1.0 because the byte ratio and the tokenizer
/// never line up exactly, and because providers round cache hits down to a block.
const MIN_SERVED_FRACTION_OF_REUSABLE: f64 = 0.5;

/// What this gateway sent for one request, kept so the provider usage counters
/// that arrive later can be judged against the prefix we know we preserved.
#[derive(Clone, Debug)]
pub(crate) struct PrefixObservation {
    provider_id: String,
    catalog_slug: String,
    upstream_model_id: String,
    protocol: &'static str,
    state: PrefixState,
    changed_regions: String,
    stable_prefix_bytes: usize,
    total_bytes: usize,
    reused_turns: usize,
    total_turns: usize,
    usage: Arc<TokenUsageAggregator>,
}

impl PrefixObservation {
    /// Tokens the provider could have served from cache on this request.
    ///
    /// Derived as the byte share of the stable prefix applied to the token count
    /// the provider itself reported, so the bytes-per-token rate cancels out. A
    /// fixed share of the whole prompt cannot work here: a turn that appends a
    /// large tool result legitimately reuses only half its prompt, while a turn
    /// that appends one line should reuse nearly all of it.
    fn reusable_tokens(&self, prompt_tokens: u64) -> u64 {
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
    fn discarded_by_provider(&self, usage: &UpstreamCacheUsage) -> bool {
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
        self.usage.record(&self.provider_id, usage);
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
    fn absorb(&mut self, usage: &Value) {
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
    fn observed(&self) -> bool {
        [self.input_tokens, self.cache_read_tokens]
            .into_iter()
            .flatten()
            .any(|value| value > 0)
    }
}

/// Holds the counters until the upstream stream is done with them. Anthropic
/// upstreams send a partial `usage` frame before the final one, so a verdict
/// taken on the first non-zero counter scores a cache hit as a miss. Reporting
/// from `Drop` covers both endings that occur in practice: the stream running to
/// completion, and a downstream mapper stopping once it sees the terminal event.
struct CacheUsageReport {
    observation: PrefixObservation,
    usage: UpstreamCacheUsage,
}

impl Drop for CacheUsageReport {
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
        let mut report = observation.map(|observation| CacheUsageReport {
            observation,
            usage: UpstreamCacheUsage::default(),
        });
        tokio::pin!(upstream);
        while let Some(chunk) = upstream.next().await {
            if let Ok(bytes) = &chunk
                && let Some(report) = report.as_mut()
            {
                for event in decoder.push(bytes) {
                    let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
                        continue;
                    };
                    for path in [
                        &data,
                        data.get("message").unwrap_or(&Value::Null),
                        data.get("response").unwrap_or(&Value::Null),
                    ] {
                        if let Some(found) = path.get("usage") {
                            report.usage.absorb(found);
                        }
                    }
                }
            }
            yield chunk;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::{ContentBlock, Message};
    use serde_json::json;

    fn responses_request(instructions: &str, input: Value, tools: Value) -> Value {
        json!({
            "model": "provider-model",
            "instructions": instructions,
            "tools": tools,
            "input": input
        })
    }

    fn turn(text: &str) -> Value {
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":text}]})
    }

    fn tracker_with_first(request: &Value) -> (CacheShapeTracker, PrefixReport) {
        let tracker = CacheShapeTracker::default();
        let report = tracker.record("session", CacheShape::from_openai_responses(request));
        (tracker, report)
    }

    fn anthropic_request() -> MessageRequest {
        MessageRequest {
            model: "provider-model".to_owned(),
            max_tokens: 64,
            stream: true,
            speed: None,
            messages: vec![Message {
                role: "user".to_owned(),
                content: vec![ContentBlock::Text {
                    text: "say hi".to_owned(),
                }],
            }],
            system: Some(vec![ContentBlock::Text {
                text: "You are Codex.".to_owned(),
            }]),
            tools: vec![json!({"name":"exec_command"})],
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    #[test]
    fn region_digests_cover_the_exact_upstream_bytes() {
        let request = anthropic_request();
        let wire = String::from_utf8(serde_json::to_vec(&request).unwrap()).unwrap();

        // The digested regions must appear verbatim in the body `reqwest`
        // serializes, otherwise the hashes describe a shape the provider never
        // sees.
        let system = serde_json::to_string(request.system.as_ref().unwrap()).unwrap();
        let message = serde_json::to_string(&request.messages[0]).unwrap();
        assert!(wire.contains(&system), "system region missing from {wire}");
        assert!(
            wire.contains(&message),
            "message region missing from {wire}"
        );

        let shape = CacheShape::from_anthropic(&request);
        assert_eq!(shape.protocol, ANTHROPIC_MESSAGES);
        assert_eq!(shape.turns.len(), 1);
        // One digest per system block, so a newly appended developer block is
        // visible as a suffix instead of a whole-region change.
        assert_eq!(
            shape.system,
            vec![RegionDigest::of(&request.system.as_ref().unwrap()[0])]
        );
        assert_eq!(shape.config, RegionDigest::ABSENT);
    }

    #[test]
    fn anthropic_thinking_and_tool_choice_are_tracked() {
        let tracker = CacheShapeTracker::default();
        let request = anthropic_request();
        tracker.record("session", CacheShape::from_anthropic(&request));

        let mut forced_tool = request.clone();
        forced_tool.tool_choice = Some(json!({"type":"tool","name":"web_search"}));
        let report = tracker.record("session", CacheShape::from_anthropic(&forced_tool));
        assert_eq!(report.state, PrefixState::ToolsChanged);

        let mut thinking = forced_tool.clone();
        thinking.thinking = Some(json!({"type":"enabled","budget_tokens":1024}));
        let report = tracker.record("session", CacheShape::from_anthropic(&thinking));
        assert_eq!(report.state, PrefixState::ConfigChanged);
    }

    /// Codex appends a developer message such as `<workspace_context>` every
    /// turn. The Anthropic converter lifts those into `system`, so the messages
    /// stay append-only while the system prompt grows.
    fn anthropic_turn(system_blocks: usize, messages: usize) -> MessageRequest {
        let mut request = anthropic_request();
        request.system = Some(
            (0..system_blocks)
                .map(|index| ContentBlock::Text {
                    text: format!("system block {index}"),
                })
                .collect(),
        );
        request.messages = (0..messages)
            .map(|index| Message {
                role: if index % 2 == 0 { "user" } else { "assistant" }.to_owned(),
                content: vec![ContentBlock::Text {
                    text: format!("turn {index}"),
                }],
            })
            .collect();
        request
    }

    /// Remote compaction replaces the transcript with a summary and rebuilds the
    /// system prompt in the same request, so both regions change at once.
    #[test]
    fn compaction_is_named_as_replaced_history_rather_than_instruction_drift() {
        let tracker = CacheShapeTracker::default();
        tracker.record(
            "session",
            CacheShape::from_anthropic(&anthropic_turn(9, 40)),
        );

        let mut compacted = anthropic_turn(1, 1);
        compacted.system = Some(vec![ContentBlock::Text {
            text: "rebuilt system prompt".to_owned(),
        }]);
        compacted.messages = vec![Message {
            role: "user".to_owned(),
            content: vec![ContentBlock::Text {
                text: "summary of the 40 earlier turns".to_owned(),
            }],
        }];
        let report = tracker.record("session", CacheShape::from_anthropic(&compacted));

        assert_eq!(report.state, PrefixState::HistoryTruncated);
        assert_eq!(report.message_prefix_turns, 0);
        assert_eq!(report.previous_turns, 40);
        // The region changes are still reported, they are just not the headline.
        assert!(report.changes.system);
    }

    /// A short session that restarts is not compaction, so it keeps the region
    /// change as its headline.
    #[test]
    fn a_short_restarted_session_is_not_reported_as_compaction() {
        let tracker = CacheShapeTracker::default();
        tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(2, 4)));

        let mut restarted = anthropic_turn(1, 1);
        restarted.system = Some(vec![ContentBlock::Text {
            text: "another system prompt".to_owned(),
        }]);
        restarted.messages = vec![Message {
            role: "user".to_owned(),
            content: vec![ContentBlock::Text {
                text: "unrelated opening turn".to_owned(),
            }],
        }];
        let report = tracker.record("session", CacheShape::from_anthropic(&restarted));

        assert_eq!(report.state, PrefixState::SystemChanged);
    }

    #[test]
    fn a_growing_system_prompt_does_not_hide_a_clean_message_history() {
        let tracker = CacheShapeTracker::default();
        tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(4, 6)));

        // One more system block and two more messages: exactly what a normal
        // Codex turn looks like today.
        let report = tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(5, 8)));

        assert_eq!(report.state, PrefixState::SystemChanged);
        assert!(report.changes.system);
        assert!(!report.changes.tools && !report.changes.config && !report.changes.model);
        assert_eq!(report.changes.as_list(), "system");
        // The provider reuses nothing, because the prompt changed at its start.
        assert_eq!(report.reused_turns, 0);
        assert_eq!(report.reused_bytes, 0);
        // The messages themselves were a clean extension, which is what tells us
        // the system prompt is the culprit rather than the history.
        assert_eq!(report.message_prefix_turns, 6);
        assert_eq!(report.previous_turns, 6);
        assert_eq!(report.total_turns, 8);
        // The first four blocks survived; block five is the new one.
        assert_eq!(report.system_prefix_blocks, 4);
        assert_eq!(report.previous_system_blocks, 4);
        assert_eq!(report.system_blocks, 5);
    }

    #[test]
    fn a_changed_system_prompt_still_reports_a_rewritten_history() {
        let tracker = CacheShapeTracker::default();
        tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(4, 6)));

        let mut rewritten = anthropic_turn(5, 8);
        rewritten.messages[1].content = vec![ContentBlock::Text {
            text: "rewritten history".to_owned(),
        }];
        let report = tracker.record("session", CacheShape::from_anthropic(&rewritten));

        // Two independent problems: the headline is the system prompt, but the
        // message prefix shows the history was also rewritten at index 1.
        assert_eq!(report.state, PrefixState::SystemChanged);
        assert!(report.changes.system);
        assert_eq!(report.message_prefix_turns, 1);
    }

    #[test]
    fn a_stable_system_prompt_with_appended_turns_keeps_the_prefix() {
        let tracker = CacheShapeTracker::default();
        tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(4, 6)));

        let report = tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(4, 8)));

        assert_eq!(report.state, PrefixState::AppendOnly);
        assert_eq!(report.changes.as_list(), "");
        assert_eq!(report.reused_turns, 6);
        assert_eq!(report.message_prefix_turns, 6);
        assert_eq!(report.system_prefix_blocks, 4);
    }

    #[test]
    fn metadata_churn_does_not_look_like_prefix_loss() {
        let tracker = CacheShapeTracker::default();
        let request = anthropic_request();
        tracker.record("session", CacheShape::from_anthropic(&request));
        let mut rerouted = request.clone();
        rerouted.metadata = Some(json!({"session_id":"another-session"}));

        let report = tracker.record("session", CacheShape::from_anthropic(&rerouted));

        assert_eq!(report.state, PrefixState::AppendOnly);
        assert_eq!(report.reused_turns, 1);
    }

    #[test]
    fn absent_region_differs_from_present_empty_region() {
        assert_ne!(RegionDigest::ABSENT, RegionDigest::of(&json!("")));
        assert_ne!(RegionDigest::ABSENT, RegionDigest::of(&json!([])));
    }

    #[test]
    fn identical_requests_produce_identical_shapes() {
        let request = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));

        let first = CacheShape::from_openai_responses(&request);
        let second = CacheShape::from_openai_responses(&request.clone());

        assert_eq!(first, second);
        assert_eq!(first.shape_hash(), second.shape_hash());
    }

    #[test]
    fn first_turn_is_a_cold_start() {
        let request = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));

        let (_, report) = tracker_with_first(&request);

        assert_eq!(report.state, PrefixState::ColdStart);
        assert!(!report.state.is_cache_loss());
        assert_eq!(report.total_turns, 1);
    }

    #[test]
    fn appended_turns_reuse_the_whole_prefix() {
        let first = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));
        let (tracker, _) = tracker_with_first(&first);
        let second = responses_request(
            "You are Codex.",
            json!([turn("say hi"), turn("and again")]),
            json!([]),
        );

        let report = tracker.record("session", CacheShape::from_openai_responses(&second));

        assert_eq!(report.state, PrefixState::AppendOnly);
        assert!(!report.state.is_cache_loss());
        assert_eq!(report.reused_turns, 1);
        assert_eq!(report.total_turns, 2);
        assert!(report.reused_bytes > 0);
    }

    #[test]
    fn rewriting_the_trailing_turn_keeps_the_earlier_prefix() {
        let first = responses_request(
            "You are Codex.",
            json!([turn("say hi"), turn("tool output")]),
            json!([]),
        );
        let (tracker, _) = tracker_with_first(&first);
        let second = responses_request(
            "You are Codex.",
            json!([turn("say hi"), turn("tool output plus more"), turn("next")]),
            json!([]),
        );

        let report = tracker.record("session", CacheShape::from_openai_responses(&second));

        assert_eq!(report.state, PrefixState::TailRewritten);
        assert!(!report.state.is_cache_loss());
        assert_eq!(report.reused_turns, 1);
    }

    #[test]
    fn rewriting_an_earlier_turn_breaks_the_prefix() {
        let first = responses_request(
            "You are Codex.",
            json!([turn("say hi"), turn("second"), turn("third")]),
            json!([]),
        );
        let (tracker, _) = tracker_with_first(&first);
        let second = responses_request(
            "You are Codex.",
            json!([
                turn("say hi"),
                turn("rewritten"),
                turn("third"),
                turn("fourth")
            ]),
            json!([]),
        );

        let report = tracker.record("session", CacheShape::from_openai_responses(&second));

        assert_eq!(report.state, PrefixState::TurnRewritten);
        assert!(report.state.is_cache_loss());
        assert_eq!(report.reused_turns, 1);
    }

    #[test]
    fn dropping_history_reports_truncation() {
        let first = responses_request(
            "You are Codex.",
            json!([turn("say hi"), turn("second"), turn("third")]),
            json!([]),
        );
        let (tracker, _) = tracker_with_first(&first);
        let second = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));

        let report = tracker.record("session", CacheShape::from_openai_responses(&second));

        assert_eq!(report.state, PrefixState::HistoryTruncated);
        assert!(report.state.is_cache_loss());
        assert_eq!(report.previous_turns, 3);
        assert_eq!(report.total_turns, 1);
    }

    #[test]
    fn instructions_and_tool_drift_are_reported_separately() {
        let first = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));
        let (tracker, _) = tracker_with_first(&first);

        let changed_system =
            responses_request("You are someone else.", json!([turn("say hi")]), json!([]));
        let report = tracker.record(
            "session",
            CacheShape::from_openai_responses(&changed_system),
        );
        assert_eq!(report.state, PrefixState::SystemChanged);
        assert_eq!(report.reused_turns, 0);

        let changed_tools = responses_request(
            "You are someone else.",
            json!([turn("say hi")]),
            json!([{"type":"function","name":"exec_command"}]),
        );
        let report = tracker.record("session", CacheShape::from_openai_responses(&changed_tools));
        assert_eq!(report.state, PrefixState::ToolsChanged);
    }

    #[test]
    fn reordering_tools_breaks_the_prefix() {
        let ordered = responses_request(
            "You are Codex.",
            json!([turn("say hi")]),
            json!([{"name":"a"},{"name":"b"}]),
        );
        let (tracker, _) = tracker_with_first(&ordered);
        let reordered = responses_request(
            "You are Codex.",
            json!([turn("say hi")]),
            json!([{"name":"b"},{"name":"a"}]),
        );

        let report = tracker.record("session", CacheShape::from_openai_responses(&reordered));

        assert_eq!(report.state, PrefixState::ToolsChanged);
    }

    #[test]
    fn reasoning_config_drift_is_reported() {
        let mut first = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));
        first["reasoning"] = json!({"effort":"medium"});
        let (tracker, _) = tracker_with_first(&first);
        let mut second = first.clone();
        second["reasoning"] = json!({"effort":"high"});

        let report = tracker.record("session", CacheShape::from_openai_responses(&second));

        assert_eq!(report.state, PrefixState::ConfigChanged);
    }

    #[test]
    fn switching_protocol_or_model_resets_the_prefix() {
        let request = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));
        let (tracker, _) = tracker_with_first(&request);

        let chat = json!({
            "model": "provider-model",
            "messages": [{"role":"system","content":"You are Codex."}]
        });
        let report = tracker.record("session", CacheShape::from_openai_chat(&chat));
        assert_eq!(report.state, PrefixState::ProtocolChanged);

        let mut other_model = chat.clone();
        other_model["model"] = json!("another-model");
        let report = tracker.record("session", CacheShape::from_openai_chat(&other_model));
        assert_eq!(report.state, PrefixState::ModelChanged);
    }

    #[test]
    fn separate_sessions_do_not_interfere() {
        let first = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));
        let (tracker, _) = tracker_with_first(&first);
        let other = responses_request("You are Codex.", json!([turn("different")]), json!([]));

        let report = tracker.record("other-session", CacheShape::from_openai_responses(&other));

        assert_eq!(report.state, PrefixState::ColdStart);
    }

    #[test]
    fn interleaved_prompts_in_one_session_are_tracked_separately() {
        // Fusion roles and concurrent subagents share a session id while sending
        // unrelated prompts. Each lineage has to keep its own prefix.
        let tracker = CacheShapeTracker::default();
        let panel = responses_request("Panel prompt.", json!([turn("say hi")]), json!([]));
        let judge = responses_request("Judge prompt.", json!([turn("judge this")]), json!([]));
        tracker.record("session", CacheShape::from_openai_responses(&panel));
        tracker.record("session", CacheShape::from_openai_responses(&judge));

        let panel_next = responses_request(
            "Panel prompt.",
            json!([turn("say hi"), turn("more")]),
            json!([]),
        );
        let report = tracker.record("session", CacheShape::from_openai_responses(&panel_next));
        assert_eq!(report.state, PrefixState::AppendOnly);
        assert_eq!(report.reused_turns, 1);

        let judge_next = responses_request(
            "Judge prompt.",
            json!([turn("judge this"), turn("again")]),
            json!([]),
        );
        let report = tracker.record("session", CacheShape::from_openai_responses(&judge_next));
        assert_eq!(report.state, PrefixState::AppendOnly);
        assert_eq!(report.reused_turns, 1);
    }

    #[test]
    fn only_the_newest_lineages_are_remembered_per_session() {
        let tracker = CacheShapeTracker::default();

        for index in 0..TRACKED_SHAPES_PER_SESSION + 3 {
            let request = responses_request(
                &format!("prompt {index}"),
                json!([turn("say hi")]),
                json!([]),
            );
            tracker.record("session", CacheShape::from_openai_responses(&request));
        }

        let sessions = tracker.sessions.lock().unwrap();
        assert_eq!(
            sessions.entries["session"].shapes.len(),
            TRACKED_SHAPES_PER_SESSION
        );
    }

    #[test]
    fn tracking_is_bounded_by_session_capacity() {
        let tracker = CacheShapeTracker::default();
        let request = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));

        for index in 0..TRACKED_SESSIONS + 8 {
            tracker.record(
                &format!("session-{index}"),
                CacheShape::from_openai_responses(&request),
            );
        }

        let sessions = tracker.sessions.lock().unwrap();
        assert_eq!(sessions.entries.len(), TRACKED_SESSIONS);
        assert!(!sessions.entries.contains_key("session-0"));
        assert!(
            sessions
                .entries
                .contains_key(&format!("session-{}", TRACKED_SESSIONS + 7))
        );
    }

    #[test]
    fn chat_shape_tracks_the_leading_system_message() {
        let first = json!({
            "model": "provider-model",
            "messages": [
                {"role":"system","content":"You are Codex."},
                {"role":"user","content":"say hi"}
            ]
        });
        let tracker = CacheShapeTracker::default();
        tracker.record("session", CacheShape::from_openai_chat(&first));
        let mut second = first.clone();
        second["messages"][0]["content"] = json!("You are someone else.");

        let report = tracker.record("session", CacheShape::from_openai_chat(&second));

        assert_eq!(report.state, PrefixState::SystemChanged);
    }

    fn observation(state: PrefixState, stable_prefix_bytes: usize) -> PrefixObservation {
        // Whole request reusable, which is what a normal appended turn looks like.
        observation_with_total(state, stable_prefix_bytes, stable_prefix_bytes.max(1))
    }

    fn observation_with_total(
        state: PrefixState,
        stable_prefix_bytes: usize,
        total_bytes: usize,
    ) -> PrefixObservation {
        PrefixObservation {
            provider_id: "baidu-oneapi".to_owned(),
            catalog_slug: "gpt-5.6-sol-baidu-oneapi".to_owned(),
            upstream_model_id: "gpt-5.6-sol".to_owned(),
            protocol: ANTHROPIC_MESSAGES,
            state,
            changed_regions: String::new(),
            stable_prefix_bytes,
            total_bytes,
            reused_turns: 40,
            total_turns: 41,
            usage: Arc::new(TokenUsageAggregator::default()),
        }
    }

    /// A turn that appends a large tool result can only reuse part of its prompt,
    /// so the verdict has to compare against the reusable part rather than the
    /// whole prompt. This is the shape that produced 182 false findings on a
    /// grok-4.5 session.
    #[test]
    fn a_turn_with_a_large_appended_tail_is_judged_against_its_reusable_prefix() {
        // Roughly half the request is the freshly appended tail.
        let subject = observation_with_total(PrefixState::AppendOnly, 74_817, 159_000);
        let healthy = UpstreamCacheUsage {
            input_tokens: Some(21_815),
            cache_read_tokens: Some(17_792),
            cache_creation_tokens: None,
            output_tokens: None,
        };

        // 45% of the prompt, but essentially all of what could be reused.
        assert!(!subject.discarded_by_provider(&healthy));

        let dropped = UpstreamCacheUsage {
            input_tokens: Some(39_479),
            cache_read_tokens: Some(128),
            cache_creation_tokens: None,
            output_tokens: None,
        };
        assert!(subject.discarded_by_provider(&dropped));
    }

    #[test]
    fn a_provider_that_drops_a_preserved_prefix_is_separated_from_our_own_cache_loss() {
        let usage = UpstreamCacheUsage {
            input_tokens: Some(95_744),
            cache_read_tokens: Some(3_456),
            cache_creation_tokens: None,
            output_tokens: None,
        };

        // Prefix kept byte-identical, provider still recomputed it.
        assert!(observation(PrefixState::AppendOnly, 960_000).discarded_by_provider(&usage));
        // Our own prefix loss already has its own warning, so this stays quiet.
        assert!(!observation(PrefixState::SystemChanged, 960_000).discarded_by_provider(&usage));
        // A cold start had no prefix to reuse in the first place.
        assert!(!observation(PrefixState::ColdStart, 0).discarded_by_provider(&usage));
        // A provider that omits the counters entirely cannot be judged.
        assert!(
            !observation(PrefixState::AppendOnly, 960_000).discarded_by_provider(
                &UpstreamCacheUsage {
                    input_tokens: Some(99_269),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    output_tokens: None,
                }
            )
        );
        // A real hit is not a finding. This shape used to be reported as a miss
        // because a bytes-per-token estimate inflated the expected prefix.
        assert!(
            !observation(PrefixState::AppendOnly, 960_000).discarded_by_provider(
                &UpstreamCacheUsage {
                    input_tokens: Some(1_391),
                    cache_read_tokens: Some(92_544),
                    cache_creation_tokens: None,
                    output_tokens: None,
                }
            )
        );
    }

    /// The system prompt and tool definitions sit ahead of the message list, so a
    /// provider cache hit has to be judged against them too.
    #[test]
    fn the_stable_prefix_spans_system_and_tools_not_only_replayed_turns() {
        let tracker = CacheShapeTracker::default();
        let first = anthropic_turn(4, 6);
        tracker.record("session", CacheShape::from_anthropic(&first));

        let report = tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(4, 8)));

        assert_eq!(report.state, PrefixState::AppendOnly);
        assert!(report.stable_prefix_bytes > report.reused_bytes);

        // A changed region invalidates from the first token, so nothing is stable.
        let mut different_system = anthropic_turn(4, 8);
        different_system.system = Some(vec![ContentBlock::Text {
            text: "another system prompt".to_owned(),
        }]);
        let report = tracker.record("session", CacheShape::from_anthropic(&different_system));

        assert!(report.state.is_cache_loss());
        assert_eq!(report.stable_prefix_bytes, 0);
    }

    #[test]
    fn provider_cache_verdicts_ignore_prefixes_too_small_to_cache() {
        let prompt_of = |prompt_tokens: u64| UpstreamCacheUsage {
            input_tokens: Some(prompt_tokens - 16),
            cache_read_tokens: Some(16),
            cache_creation_tokens: None,
            output_tokens: None,
        };

        let subject = observation(PrefixState::AppendOnly, 960_000);
        assert!(
            !subject.discarded_by_provider(&prompt_of(MIN_PROMPT_TOKENS_FOR_VERDICT - 1)),
            "a prompt below the cacheable floor says nothing about the shape"
        );
        assert!(subject.discarded_by_provider(&prompt_of(MIN_PROMPT_TOKENS_FOR_VERDICT)));
    }

    #[tokio::test]
    async fn cache_usage_observation_relays_bytes_untouched() {
        let events = vec![
            Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":240000,\"cache_read_input_tokens\":3456}}}\n\n",
            ),
            Bytes::from_static(
                b"event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":46}}\n\n",
            ),
        ];
        let upstream = futures_util::stream::iter(events.clone().into_iter().map(Ok));

        let relayed: Vec<Bytes> = observe_upstream_cache_usage(
            upstream,
            Some(observation(PrefixState::AppendOnly, 960_000)),
        )
        .map(Result::unwrap)
        .collect()
        .await;

        assert_eq!(relayed, events);
    }

    #[tokio::test]
    async fn cache_usage_observation_reads_responses_completed_usage() {
        let events = vec![
            Bytes::from_static(
                b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":4,\"input_tokens_details\":{\"cached_tokens\":3},\"output_tokens\":2}}}\n\n",
            ),
        ];
        let usage = Arc::new(TokenUsageAggregator::default());
        let upstream = futures_util::stream::iter(events.clone().into_iter().map(Ok));
        let mut observation = observation(PrefixState::AppendOnly, 960_000);
        observation.usage = usage.clone();

        let relayed: Vec<Bytes> = observe_upstream_cache_usage(upstream, Some(observation))
            .map(Result::unwrap)
            .collect()
            .await;

        assert_eq!(relayed, events);
        let snapshot = usage.snapshot();
        assert_eq!(snapshot[0].input_tokens, 1);
        assert_eq!(snapshot[0].cache_read_tokens, 3);
        assert_eq!(snapshot[0].output_tokens, 2);
    }

    #[test]
    fn usage_counters_are_absorbed_from_both_message_and_top_level_shapes() {
        let mut usage = UpstreamCacheUsage::default();
        // Anthropic opens with a zeroed `usage`, which must not count as observed.
        usage.absorb(&json!({"input_tokens": 0, "output_tokens": 0}));
        assert!(!usage.observed());

        usage.absorb(&json!({"input_tokens": 100, "cache_read_input_tokens": 3456}));
        usage.absorb(&json!({"cache_creation_input_tokens": 7}));
        usage.absorb(&json!({"output_tokens": 42}));

        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.cache_read_tokens, Some(3456));
        assert_eq!(usage.cache_creation_tokens, Some(7));
        assert_eq!(usage.output_tokens, Some(42));
        assert!(usage.observed());
    }

    /// Baidu OneAPI reports an optimistic partial count on `message_start` and
    /// the real one on `message_delta`. Only the last frame is the verdict.
    #[test]
    fn a_later_usage_frame_replaces_an_earlier_partial_one() {
        let mut usage = UpstreamCacheUsage::default();
        usage.absorb(&json!({"input_tokens": 15_744, "cache_read_input_tokens": 123_021}));
        let subject = observation(PrefixState::AppendOnly, 960_000);
        assert!(
            !subject.discarded_by_provider(&usage),
            "the partial frame looks like a hit"
        );

        usage.absorb(&json!({"input_tokens": 135_947, "cache_read_input_tokens": 3_456}));

        assert_eq!(usage.cache_read_tokens, Some(3_456));
        assert_eq!(usage.input_tokens, Some(135_947));
        assert!(
            subject.discarded_by_provider(&usage),
            "the final frame is the verdict"
        );
    }

    #[test]
    fn token_usage_aggregator_sums_provider_counters_and_cache_hit_rate() {
        let aggregator = TokenUsageAggregator::default();
        aggregator.record(
            "baidu-oneapi",
            &UpstreamCacheUsage {
                input_tokens: Some(1_000),
                cache_read_tokens: Some(3_000),
                cache_creation_tokens: Some(500),
                output_tokens: Some(200),
            },
        );
        aggregator.record(
            "baidu-oneapi",
            &UpstreamCacheUsage {
                input_tokens: Some(500),
                cache_read_tokens: Some(1_500),
                cache_creation_tokens: None,
                output_tokens: Some(100),
            },
        );

        let snapshot = aggregator.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].provider_id, "baidu-oneapi");
        assert_eq!(snapshot[0].request_count, 2);
        assert_eq!(snapshot[0].input_tokens, 1_500);
        assert_eq!(snapshot[0].cache_read_tokens, 4_500);
        assert_eq!(snapshot[0].cache_creation_tokens, 500);
        assert_eq!(snapshot[0].output_tokens, 300);
        assert_eq!(
            snapshot[0].cache_hit_percent,
            Some(4_500.0 / 6_500.0 * 100.0)
        );
    }

    /// Chat Completions upstreams report cache hits under their own names, and
    /// `prompt_tokens` includes them, so the uncached part has to be derived.
    #[test]
    fn chat_and_responses_cache_counters_normalise_to_uncached_input() {
        let mut deepseek = UpstreamCacheUsage::default();
        deepseek.absorb(
            &json!({"prompt_tokens": 12_000, "prompt_cache_hit_tokens": 11_500, "prompt_cache_miss_tokens": 500}),
        );
        assert_eq!(deepseek.cache_read_tokens, Some(11_500));
        assert_eq!(deepseek.input_tokens, Some(500));

        let mut chat = UpstreamCacheUsage::default();
        chat.absorb(
            &json!({"prompt_tokens": 12_000, "prompt_tokens_details": {"cached_tokens": 9_000}}),
        );
        assert_eq!(chat.cache_read_tokens, Some(9_000));
        assert_eq!(chat.input_tokens, Some(3_000));

        let mut responses = UpstreamCacheUsage::default();
        responses.absorb(
            &json!({"input_tokens": 4_000, "input_tokens_details": {"cached_tokens": 3_500}}),
        );
        assert_eq!(responses.cache_read_tokens, Some(3_500));
        assert_eq!(responses.input_tokens, Some(500));
    }
}
