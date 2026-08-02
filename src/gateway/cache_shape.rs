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
use std::sync::{Mutex, PoisonError};

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

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

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
        PrefixReport {
            state: changes
                .state()
                .or(turn_state)
                .unwrap_or(PrefixState::AppendOnly),
            changes,
            reused_turns,
            reused_bytes: self.turns[..reused_turns]
                .iter()
                .map(|turn| turn.bytes)
                .sum(),
            stable_prefix_bytes: self.stable_prefix_bytes(previous, &changes, reused_turns),
            message_prefix_turns,
            previous_turns: previous.turns.len(),
            total_turns: self.turns.len(),
            system_prefix_blocks: common_prefix_len(&self.system, &previous.system),
            previous_system_blocks: previous.system.len(),
            system_blocks: self.system.len(),
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
}

impl CacheShapeTracker {
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
        let report = tracked
            .shapes
            .iter()
            .map(|earlier| shape.compare(earlier))
            .max_by_key(|report| (report.reused_turns, !report.state.is_cache_loss()))
            .unwrap_or(PrefixReport {
                state: PrefixState::ColdStart,
                changes: PrefixChanges::default(),
                reused_turns: 0,
                reused_bytes: 0,
                stable_prefix_bytes: 0,
                message_prefix_turns: 0,
                previous_turns: 0,
                total_turns: shape.turns.len(),
                system_prefix_blocks: 0,
                previous_system_blocks: 0,
                system_blocks: shape.system.len(),
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
        reused_turns: report.reused_turns,
        total_turns: report.total_turns,
    })
}

/// Average bytes per token across the prompt shapes Codex sends. Only used to
/// turn a byte-level reuse measurement into a token-level expectation that can
/// be compared against provider usage counters.
const BYTES_PER_TOKEN_ESTIMATE: usize = 4;

/// A stable prefix below this token estimate is too small for provider caches to
/// act on, so a miss there says nothing about the request shape.
const MIN_REUSED_TOKENS_FOR_VERDICT: usize = 8_192;

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
    reused_turns: usize,
    total_turns: usize,
}

impl PrefixObservation {
    /// True when this gateway kept a substantial prefix byte-identical and the
    /// provider still recomputed most of it.
    fn discarded_by_provider(&self, usage: &UpstreamCacheUsage) -> bool {
        let stable_tokens_estimate = self.stable_prefix_bytes / BYTES_PER_TOKEN_ESTIMATE;
        if self.state.is_cache_loss() || stable_tokens_estimate < MIN_REUSED_TOKENS_FOR_VERDICT {
            return false;
        }
        let cache_read_tokens =
            usize::try_from(usage.cache_read_tokens.unwrap_or(0)).unwrap_or(usize::MAX);
        cache_read_tokens < stable_tokens_estimate / 2
    }

    /// Logs the provider cache counters next to the prefix this gateway
    /// preserved. A provider that recomputes a prefix we kept byte-identical is
    /// the only case that warrants a warning, because nothing on this side can
    /// fix it and it otherwise looks like a gateway bug.
    pub(crate) fn report_upstream_cache(&self, usage: &UpstreamCacheUsage) {
        let stable_tokens_estimate = self.stable_prefix_bytes / BYTES_PER_TOKEN_ESTIMATE;
        let cache_read_tokens = usage.cache_read_tokens.unwrap_or(0);
        if self.discarded_by_provider(usage) {
            tracing::warn!(
                provider_id = self.provider_id,
                catalog_slug = self.catalog_slug,
                upstream_model_id = self.upstream_model_id,
                protocol = self.protocol,
                prefix_state = self.state.as_str(),
                reused_turns = self.reused_turns,
                total_turns = self.total_turns,
                stable_prefix_tokens_estimate = stable_tokens_estimate,
                cache_read_tokens,
                uncached_input_tokens = usage.input_tokens.unwrap_or(0),
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
            stable_prefix_tokens_estimate = stable_tokens_estimate,
            cache_read_tokens,
            uncached_input_tokens = usage.input_tokens.unwrap_or(0),
            cache_creation_tokens = usage.cache_creation_tokens.unwrap_or(0),
            "provider prompt cache usage"
        );
    }
}

/// Provider cache counters observed on an Anthropic Messages response.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct UpstreamCacheUsage {
    pub(crate) input_tokens: Option<u64>,
    pub(crate) cache_read_tokens: Option<u64>,
    pub(crate) cache_creation_tokens: Option<u64>,
}

impl UpstreamCacheUsage {
    fn absorb(&mut self, usage: &Value) {
        for (field, slot) in [
            ("input_tokens", &mut self.input_tokens),
            ("cache_read_input_tokens", &mut self.cache_read_tokens),
            (
                "cache_creation_input_tokens",
                &mut self.cache_creation_tokens,
            ),
        ] {
            if let Some(value) = usage.get(field).and_then(Value::as_u64) {
                *slot = Some(value);
            }
        }
    }

    fn observed(&self) -> bool {
        self.input_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_creation_tokens.is_some()
    }
}

/// Passes an Anthropic Messages byte stream through untouched while collecting
/// the prompt cache counters, then reports them against the recorded prefix.
/// Reading the counters here keeps the downstream event mappers unaware of
/// diagnostics, and works for every route that speaks Messages upstream.
pub(crate) fn observe_anthropic_cache_usage<S>(
    upstream: S,
    observation: Option<PrefixObservation>,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    async_stream::stream! {
        let mut decoder = SseDecoder::default();
        let mut usage = UpstreamCacheUsage::default();
        // Reported as soon as the counters arrive, because a downstream mapper
        // may stop reading once it sees `message_stop` and drop this stream
        // before its tail runs.
        let mut reported = false;
        tokio::pin!(upstream);
        while let Some(chunk) = upstream.next().await {
            if let Ok(bytes) = &chunk
                && let Some(observation) = observation.as_ref()
                && !reported
            {
                for event in decoder.push(bytes) {
                    let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
                        continue;
                    };
                    for path in [&data, data.get("message").unwrap_or(&Value::Null)] {
                        if let Some(found) = path.get("usage") {
                            usage.absorb(found);
                        }
                    }
                }
                if usage.observed() {
                    observation.report_upstream_cache(&usage);
                    reported = true;
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
        PrefixObservation {
            provider_id: "baidu-oneapi".to_owned(),
            catalog_slug: "gpt-5.6-sol-baidu-oneapi".to_owned(),
            upstream_model_id: "gpt-5.6-sol".to_owned(),
            protocol: ANTHROPIC_MESSAGES,
            state,
            changed_regions: String::new(),
            stable_prefix_bytes,
            reused_turns: 40,
            total_turns: 41,
        }
    }

    #[test]
    fn a_provider_that_drops_a_preserved_prefix_is_separated_from_our_own_cache_loss() {
        let usage = UpstreamCacheUsage {
            input_tokens: Some(240_000),
            cache_read_tokens: Some(3_456),
            cache_creation_tokens: None,
        };

        // Prefix kept byte-identical, provider still recomputed it.
        assert!(observation(PrefixState::AppendOnly, 960_000).discarded_by_provider(&usage));
        // Our own prefix loss already has its own warning, so this stays quiet.
        assert!(!observation(PrefixState::SystemChanged, 960_000).discarded_by_provider(&usage));
        // Too little stable prefix for a provider cache to act on.
        assert!(!observation(PrefixState::AppendOnly, 8_000).discarded_by_provider(&usage));
        // A real hit is not a finding.
        assert!(
            !observation(PrefixState::AppendOnly, 960_000).discarded_by_provider(
                &UpstreamCacheUsage {
                    input_tokens: Some(1_800),
                    cache_read_tokens: Some(238_000),
                    cache_creation_tokens: None,
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
        let usage = UpstreamCacheUsage {
            input_tokens: Some(240_000),
            cache_read_tokens: Some(3_456),
            cache_creation_tokens: None,
        };

        let boundary = MIN_REUSED_TOKENS_FOR_VERDICT * BYTES_PER_TOKEN_ESTIMATE;
        assert!(!observation(PrefixState::AppendOnly, boundary - 4).discarded_by_provider(&usage));
        assert!(observation(PrefixState::AppendOnly, boundary).discarded_by_provider(&usage));
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

        let relayed: Vec<Bytes> = observe_anthropic_cache_usage(
            upstream,
            Some(observation(PrefixState::AppendOnly, 960_000)),
        )
        .map(Result::unwrap)
        .collect()
        .await;

        assert_eq!(relayed, events);
    }

    #[test]
    fn usage_counters_are_absorbed_from_both_message_and_top_level_shapes() {
        let mut usage = UpstreamCacheUsage::default();
        usage.absorb(&json!({"input_tokens": 100, "cache_read_input_tokens": 3456}));
        usage.absorb(&json!({"cache_creation_input_tokens": 7}));

        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.cache_read_tokens, Some(3456));
        assert_eq!(usage.cache_creation_tokens, Some(7));
        assert!(usage.observed());
    }
}
