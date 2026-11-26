//! Protocol vocabulary shared by the conversion layer and everything above it.
//!
//! These types live at the bottom of the dependency graph: `protocol` produces
//! and consumes them, and the layers above (`provider`, `config`) re-export the
//! same definitions instead of owning their own copies.

use serde::{Deserialize, Serialize};

/// Wire protocol a provider endpoint speaks.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
    GoogleGemini,
}

/// How the gateway chooses a model's thinking budget for one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingMode {
    Off,
    Auto,
}
