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

use serde::Serialize;
use serde_json::Value;

use crate::anthropic::MessageRequest;
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
    /// System prompt carrier: Anthropic `system`, Responses `instructions`, or
    /// the leading `system` chat message.
    system: RegionDigest,
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
            system: request
                .system
                .as_ref()
                .map_or(RegionDigest::ABSENT, RegionDigest::of),
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
                .map_or(RegionDigest::ABSENT, RegionDigest::of),
            tools: RegionDigest::of(&(request.get("tools"), request.get("tool_choice"))),
            config: RegionDigest::ABSENT,
            turns: turn_digests(messages),
        }
    }

    pub(crate) fn from_openai_responses(request: &Value) -> Self {
        Self {
            protocol: OPENAI_RESPONSES,
            model: request_model(request),
            system: RegionDigest::of_optional(request.get("instructions")),
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
            "{}|{}|{}|{}|{}",
            self.protocol, self.model, self.system.hash, self.tools.hash, self.config.hash
        );
        for turn in &self.turns {
            let _ = write!(hasher, "|{}", turn.hash);
        }
        hasher.hash
    }

    fn compare(&self, previous: &Self) -> PrefixReport {
        let report = |state, reused_turns: usize| PrefixReport {
            state,
            reused_turns,
            reused_bytes: self.turns[..reused_turns]
                .iter()
                .map(|turn| turn.bytes)
                .sum(),
            previous_turns: previous.turns.len(),
            total_turns: self.turns.len(),
        };
        if previous.protocol != self.protocol {
            return report(PrefixState::ProtocolChanged, 0);
        }
        if previous.model != self.model {
            return report(PrefixState::ModelChanged, 0);
        }
        if previous.system != self.system {
            return report(PrefixState::SystemChanged, 0);
        }
        if previous.tools != self.tools {
            return report(PrefixState::ToolsChanged, 0);
        }
        if previous.config != self.config {
            return report(PrefixState::ConfigChanged, 0);
        }
        let reused_turns = self
            .turns
            .iter()
            .zip(&previous.turns)
            .take_while(|(current, earlier)| current == earlier)
            .count();
        if reused_turns == previous.turns.len() {
            return report(PrefixState::AppendOnly, reused_turns);
        }
        if reused_turns == self.turns.len() {
            return report(PrefixState::HistoryTruncated, reused_turns);
        }
        // Converters legitimately merge a freshly appended tool result into the
        // previous trailing message, so only the final earlier turn moves. Every
        // turn before it still caches.
        if reused_turns + 1 == previous.turns.len() {
            return report(PrefixState::TailRewritten, reused_turns);
        }
        report(PrefixState::TurnRewritten, reused_turns)
    }
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
    pub(crate) reused_turns: usize,
    pub(crate) reused_bytes: usize,
    pub(crate) previous_turns: usize,
    pub(crate) total_turns: usize,
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
                reused_turns: 0,
                reused_bytes: 0,
                previous_turns: 0,
                total_turns: shape.turns.len(),
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
) {
    let Some(routing) = routing else {
        tracing::debug!(
            provider_id,
            catalog_slug,
            upstream_model_id,
            protocol = shape.protocol,
            "provider request has no session key, so prefix cache tracking is unavailable"
        );
        return;
    };
    // Fusion panels and judges share one Codex session while sending different
    // prompts, so the tracked key has to include the upstream model.
    let session_key = format!(
        "{}|{provider_id}|{catalog_slug}|{upstream_model_id}",
        routing.session_id
    );
    let protocol = shape.protocol;
    let shape_hash = shape.shape_hash();
    let system_hash = shape.system.hash;
    let tools_hash = shape.tools.hash;
    let report = tracker.record(&session_key, shape);
    if report.state.is_cache_loss() {
        tracing::warn!(
            provider_id,
            catalog_slug,
            upstream_model_id,
            protocol,
            prefix_state = report.state.as_str(),
            reused_turns = report.reused_turns,
            reused_bytes = report.reused_bytes,
            previous_turns = report.previous_turns,
            total_turns = report.total_turns,
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
            reused_turns = report.reused_turns,
            reused_bytes = report.reused_bytes,
            previous_turns = report.previous_turns,
            total_turns = report.total_turns,
            shape_hash,
            system_hash,
            tools_hash,
            "provider prompt prefix cache shape"
        );
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
        assert_eq!(
            shape.system,
            RegionDigest::of(request.system.as_ref().unwrap())
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
}
