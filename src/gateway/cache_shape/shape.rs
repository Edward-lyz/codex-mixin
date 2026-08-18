use std::io::Write;

use serde::Serialize;
use serde_json::Value;

use crate::anthropic::MessageRequest;

pub(crate) const ANTHROPIC_MESSAGES: &str = "anthropic_messages";
pub(crate) const OPENAI_CHAT: &str = "openai_chat";
pub(crate) const OPENAI_RESPONSES: &str = "openai_responses";

/// History this long, replaced wholesale by a much shorter one, is compaction
/// rather than prompt drift. Below this the two are indistinguishable from a
/// short session that simply restarted.
const MIN_TURNS_FOR_REPLACED_HISTORY: usize = 8;

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
    pub(super) hash: u64,
    pub(super) bytes: usize,
}

impl RegionDigest {
    /// Region the protocol does not send at all. Distinct from a region that is
    /// present but empty, because those affect the prompt differently.
    pub(super) const ABSENT: Self = Self { hash: 0, bytes: 0 };

    pub(super) fn of<T: Serialize + ?Sized>(value: &T) -> Self {
        let mut hasher = ShapeHasher::new();
        if serde_json::to_writer(&mut hasher, value).is_err() {
            return Self::ABSENT;
        }
        Self {
            hash: hasher.hash,
            bytes: hasher.bytes,
        }
    }

    pub(super) fn of_optional(value: Option<&Value>) -> Self {
        value.map_or(Self::ABSENT, Self::of)
    }
}

/// Cache-relevant regions of a single provider-visible request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CacheShape {
    pub(super) protocol: &'static str,
    pub(super) model: String,
    /// System prompt carrier, one digest per block: Anthropic `system` blocks,
    /// Responses `instructions`, or the leading `system` chat message.
    ///
    /// Kept per block because Codex appends a developer message such as
    /// `<workspace_context>` every turn. A single digest would only say the
    /// system prompt moved, not which block moved it.
    pub(super) system: Vec<RegionDigest>,
    /// Tool configuration, covering both the definitions and `tool_choice`,
    /// because either one shifts the cached tool preamble.
    pub(super) tools: RegionDigest,
    /// How many tool definitions the digest covers.
    pub(super) tools_count: usize,
    /// Reasoning configuration, which providers fold into the cached prefix.
    pub(super) config: RegionDigest,
    /// One digest per message, in wire order.
    pub(super) turns: Vec<RegionDigest>,
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
    pub(super) fn shape_hash(&self) -> u64 {
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

    pub(super) fn system_hash(&self) -> u64 {
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
    pub(super) fn total_bytes(&self) -> usize {
        self.system.iter().map(|block| block.bytes).sum::<usize>()
            + self.tools.bytes
            + self.config.bytes
            + self.turns.iter().map(|turn| turn.bytes).sum::<usize>()
    }

    /// Number of tool definitions is not recoverable from a digest, so it is
    /// tracked separately: a tool set that changes hash while keeping its size
    /// points at ordering or field drift, which is fixable here, while a changed
    /// size means the client really added or removed tools.
    pub(super) fn tools_count(&self) -> usize {
        self.tools_count
    }

    /// Compares every region instead of returning on the first difference.
    ///
    /// A changed system prompt already costs the whole prefix, but stopping
    /// there also hides an independent problem in the message sequence.
    pub(super) fn compare(&self, previous: &Self) -> PrefixReport {
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
