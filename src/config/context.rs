use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::context::types::{
    DEFAULT_GROWTH_THRESHOLD_TOKENS, DEFAULT_MAX_INTERNAL_ROUNDS, DEFAULT_PRESSURE_RATIO,
    DEFAULT_PROTECTED_MESSAGES, DEFAULT_PROTECTED_TOKENS, DEFAULT_URGENT_RATIO,
};

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
    /// Explicit model context window override. Real model metadata wins when
    /// the gateway knows it.
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
        if self.max_internal_rounds == 0 || self.max_internal_rounds > 8 {
            bail!("context max_internal_rounds must be between 1 and 8");
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
