use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Default context-compression thresholds, owned by the configuration layer so
/// settings validation never has to depend on the compression engine.
pub const DEFAULT_GROWTH_THRESHOLD_TOKENS: u64 = 100_000;
pub const DEFAULT_PRESSURE_RATIO: f64 = 0.75;
pub const DEFAULT_URGENT_RATIO: f64 = 0.95;
pub const DEFAULT_PROTECTED_MESSAGES: usize = 3;
pub const DEFAULT_PROTECTED_TOKENS: u64 = 10_000;
pub const DEFAULT_MAX_INTERNAL_ROUNDS: usize = 10;
pub const DEFAULT_MAX_TIER: u8 = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextSettings {
    /// Global switch. Off stops new compression but keeps archived sessions
    /// readable, searchable and restorable.
    pub enabled: bool,
    pub growth_threshold_tokens: u64,
    pub pressure_ratio: f64,
    pub urgent_ratio: f64,
    pub protected_messages: usize,
    pub protected_tokens: u64,
    pub max_internal_rounds: usize,
    /// Highest compression tier the engine may produce. Lowering it keeps the
    /// ablation arm that only allows tier 1 summaries reproducible.
    pub max_tier: u8,
    /// Injects the compression guidance without exposing the compression
    /// tools, which is the guidance-only ablation arm.
    pub guidance_only: bool,
    /// Explicit view window override, including evaluation windows. When unset,
    /// the gateway resolves the model's metadata before using its default.
    pub context_window_tokens: Option<u64>,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            growth_threshold_tokens: DEFAULT_GROWTH_THRESHOLD_TOKENS,
            pressure_ratio: DEFAULT_PRESSURE_RATIO,
            urgent_ratio: DEFAULT_URGENT_RATIO,
            protected_messages: DEFAULT_PROTECTED_MESSAGES,
            protected_tokens: DEFAULT_PROTECTED_TOKENS,
            max_internal_rounds: DEFAULT_MAX_INTERNAL_ROUNDS,
            max_tier: DEFAULT_MAX_TIER,
            guidance_only: false,
            context_window_tokens: None,
        }
    }
}

impl ContextSettings {
    pub fn validate(&self) -> Result<()> {
        if !(0.0..=1.0).contains(&self.pressure_ratio) || self.pressure_ratio == 0.0 {
            bail!("context pressure_ratio must be in (0, 1]");
        }
        if !(0.0..=1.0).contains(&self.urgent_ratio) || self.urgent_ratio == 0.0 {
            bail!("context urgent_ratio must be in (0, 1]");
        }
        if self.pressure_ratio >= self.urgent_ratio {
            bail!("context pressure_ratio must be below urgent_ratio");
        }
        if self.growth_threshold_tokens == 0 {
            bail!("context growth_threshold_tokens must be positive");
        }
        if self.protected_messages == 0 {
            bail!("context protected_messages must be positive");
        }
        if self.protected_tokens == 0 {
            bail!("context protected_tokens must be positive");
        }
        if self.max_internal_rounds == 0 || self.max_internal_rounds > DEFAULT_MAX_INTERNAL_ROUNDS {
            bail!("context max_internal_rounds must be between 1 and 10");
        }
        if self.max_tier == 0 || self.max_tier > 3 {
            bail!("context max_tier must be between 1 and 3");
        }
        if self.context_window_tokens == Some(0) {
            bail!("context context_window_tokens must be positive when set");
        }
        Ok(())
    }
}

impl ContextSettings {
    pub fn is_default(settings: &Self) -> bool {
        *settings == Self::default()
    }
}
