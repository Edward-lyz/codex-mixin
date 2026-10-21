use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_GROWTH_THRESHOLD_TOKENS: u64 = 50_000;
pub const DEFAULT_PRESSURE_RATIO: f64 = 0.75;
pub const DEFAULT_URGENT_RATIO: f64 = 0.95;
pub const DEFAULT_PROTECTED_MESSAGES: usize = 5;
pub const DEFAULT_PROTECTED_TOKENS: u64 = 5_000;
pub const DEFAULT_MAX_INTERNAL_ROUNDS: usize = 4;

pub const TOOL_COMPRESS: &str = "mixin_context_compress";
pub const TOOL_DECOMPRESS: &str = "mixin_context_decompress";
pub const TOOL_SEARCH: &str = "mixin_context_search";
pub const TOOL_STATUS: &str = "mixin_context_status";

pub fn is_context_tool(name: &str) -> bool {
    matches!(
        name,
        TOOL_COMPRESS | TOOL_DECOMPRESS | TOOL_SEARCH | TOOL_STATUS
    )
}
/// Approximate token count for mixed English and CJK text.
///
/// The estimator is cheap and deterministic: English averages roughly four
/// characters per token, CJK roughly two thirds of a token per character.
/// Real billed usage is recorded separately for calibration.
pub fn estimate_text_tokens(text: &str) -> u64 {
    let mut ascii = 0u64;
    let mut wide = 0u64;
    for character in text.chars() {
        if character.is_ascii() {
            ascii += 1;
        } else {
            wide += 1;
        }
    }
    let ascii_tokens = ascii.div_ceil(4);
    let wide_tokens = wide.saturating_mul(2).div_ceil(3);
    ascii_tokens.saturating_add(wide_tokens).max(1)
}

pub fn estimate_item_tokens(item: &Value) -> u64 {
    estimate_text_tokens(&item.to_string()).saturating_add(4)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    One,
    Two,
    Three,
}

impl Tier {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
        }
    }

    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::One),
            2 => Some(Self::Two),
            3 => Some(Self::Three),
            _ => None,
        }
    }

    pub fn next(self) -> Option<Self> {
        match self {
            Self::One => Some(Self::Two),
            Self::Two => Some(Self::Three),
            Self::Three => Some(Self::Three),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    Message,
    Block,
}

impl SegmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Block => "block",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "message" => Some(Self::Message),
            "block" => Some(Self::Block),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DirectRef {
    pub kind: SegmentKind,
    pub reference: String,
    pub position: i64,
    pub end_position: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Protection {
    /// Segment references that must not be compressed.
    pub protected: Vec<String>,
    /// Human-readable reasons, aligned with the protected list.
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TierCounts {
    pub messages: u64,
    pub t1_blocks: u64,
    pub t2_blocks: u64,
    pub t3_blocks: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompressibleRange {
    pub start: String,
    pub end: String,
    pub tier: String,
    pub estimated_tokens: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextStatus {
    pub session: String,
    pub revision: i64,
    pub enabled: bool,
    pub window_tokens: u64,
    pub estimated_view_tokens: u64,
    pub growth_threshold_tokens: u64,
    pub pressure: f64,
    pub tiers: TierCounts,
    pub archive_messages: u64,
    pub archive_bytes: u64,
    pub protected: Vec<String>,
    pub compressible: Vec<CompressibleRange>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session: String,
    pub revision: i64,
    pub updated_at: i64,
    pub messages: u64,
    pub blocks: u64,
}

#[derive(Clone, Debug)]
pub struct TurnView {
    pub revision: i64,
    pub items: Vec<Value>,
    pub estimated_tokens: u64,
    pub window_tokens: u64,
    pub protection: Protection,
}

impl TurnView {
    pub fn pressure(&self) -> f64 {
        if self.window_tokens == 0 {
            return 0.0;
        }
        self.estimated_tokens as f64 / self.window_tokens as f64
    }
}

pub const SESSION_SEPARATOR: char = '\u{1f}';

pub fn session_display_name(session: &str) -> String {
    session.replace(SESSION_SEPARATOR, " / ")
}
