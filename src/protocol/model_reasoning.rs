use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReasoningLevel {
    pub effort: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnthropicThinkingKind {
    Adaptive,
    Manual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelReasoningCapabilities {
    pub default_effort: &'static str,
    pub supported_levels: &'static [ReasoningLevel],
    pub multi_agent_version: Option<&'static str>,
    pub anthropic_thinking: Option<AnthropicThinkingKind>,
}

const LOW: ReasoningLevel = ReasoningLevel {
    effort: "low",
    description: "Fast responses with lighter reasoning",
};
const OFF: ReasoningLevel = ReasoningLevel {
    effort: "none",
    description: "Disable reasoning",
};
const MEDIUM: ReasoningLevel = ReasoningLevel {
    effort: "medium",
    description: "Balances speed and reasoning depth for everyday tasks",
};
const HIGH: ReasoningLevel = ReasoningLevel {
    effort: "high",
    description: "Greater reasoning depth for complex problems",
};
const XHIGH: ReasoningLevel = ReasoningLevel {
    effort: "xhigh",
    description: "Extra high reasoning depth for complex problems",
};
const MAX: ReasoningLevel = ReasoningLevel {
    effort: "max",
    description: "Maximum reasoning depth for the hardest problems",
};
const ULTRA: ReasoningLevel = ReasoningLevel {
    effort: "ultra",
    description: "Maximum reasoning with automatic task delegation",
};

const DEFAULT_LEVELS: &[ReasoningLevel] = &[OFF, LOW, MEDIUM, HIGH, XHIGH, MAX, ULTRA];
const ULTRA_ONLY_LEVELS: &[ReasoningLevel] = &[OFF, ULTRA];

const GPT_5_6_SOL: ModelReasoningCapabilities = ModelReasoningCapabilities {
    default_effort: "low",
    supported_levels: DEFAULT_LEVELS,
    multi_agent_version: Some("v2"),
    anthropic_thinking: None,
};
const GPT_5_6_TERRA: ModelReasoningCapabilities = ModelReasoningCapabilities {
    default_effort: "medium",
    supported_levels: DEFAULT_LEVELS,
    multi_agent_version: Some("v2"),
    anthropic_thinking: None,
};
const GPT_5_6_LUNA: ModelReasoningCapabilities = ModelReasoningCapabilities {
    default_effort: "medium",
    supported_levels: DEFAULT_LEVELS,
    multi_agent_version: Some("v2"),
    anthropic_thinking: None,
};
const GPT_5_5: ModelReasoningCapabilities = ModelReasoningCapabilities {
    default_effort: "medium",
    supported_levels: DEFAULT_LEVELS,
    multi_agent_version: Some("v2"),
    anthropic_thinking: None,
};
const GPT_5_4: ModelReasoningCapabilities = ModelReasoningCapabilities {
    default_effort: "medium",
    supported_levels: DEFAULT_LEVELS,
    multi_agent_version: Some("v2"),
    anthropic_thinking: None,
};
const CLAUDE_ADAPTIVE: ModelReasoningCapabilities = ModelReasoningCapabilities {
    default_effort: "medium",
    supported_levels: DEFAULT_LEVELS,
    multi_agent_version: Some("v2"),
    anthropic_thinking: Some(AnthropicThinkingKind::Adaptive),
};
const CLAUDE_MANUAL: ModelReasoningCapabilities = ModelReasoningCapabilities {
    default_effort: "medium",
    supported_levels: DEFAULT_LEVELS,
    multi_agent_version: Some("v2"),
    anthropic_thinking: Some(AnthropicThinkingKind::Manual),
};
const DEFAULT_REASONING: ModelReasoningCapabilities = ModelReasoningCapabilities {
    default_effort: "medium",
    supported_levels: DEFAULT_LEVELS,
    multi_agent_version: Some("v2"),
    anthropic_thinking: None,
};
const PROVIDER_ADVERTISED_REASONING: ModelReasoningCapabilities = ModelReasoningCapabilities {
    anthropic_thinking: Some(AnthropicThinkingKind::Adaptive),
    ..DEFAULT_REASONING
};
const ULTRA_ONLY_REASONING: ModelReasoningCapabilities = ModelReasoningCapabilities {
    default_effort: "none",
    supported_levels: ULTRA_ONLY_LEVELS,
    multi_agent_version: Some("v2"),
    anthropic_thinking: None,
};

pub fn detect_model_reasoning(model: &str) -> Option<ModelReasoningCapabilities> {
    let model = model.trim().to_ascii_lowercase();
    if model_or_provider_qualified(&model, "gpt-5.6-terra") {
        return Some(GPT_5_6_TERRA);
    }
    if model_or_provider_qualified(&model, "gpt-5.6-luna") {
        return Some(GPT_5_6_LUNA);
    }
    if model_or_provider_qualified(&model, "gpt-5.6-sol")
        || model_or_provider_qualified(&model, "gpt-5.6")
    {
        return Some(GPT_5_6_SOL);
    }
    if model_or_provider_qualified(&model, "gpt-5.5") {
        return Some(GPT_5_5);
    }
    if model_or_provider_qualified(&model, "gpt-5.4")
        || model_or_provider_qualified(&model, "gpt-5.4-mini")
    {
        return Some(GPT_5_4);
    }
    if uses_adaptive_anthropic_thinking(&model) {
        return Some(CLAUDE_ADAPTIVE);
    }
    if uses_manual_anthropic_thinking(&model) {
        return Some(CLAUDE_MANUAL);
    }
    None
}

pub fn resolve_model_reasoning(
    model: &str,
    advertised_support: Option<bool>,
) -> Option<ModelReasoningCapabilities> {
    if advertised_support == Some(false) {
        return Some(ULTRA_ONLY_REASONING);
    }
    Some(
        detect_model_reasoning(model).unwrap_or(match advertised_support {
            Some(true) => PROVIDER_ADVERTISED_REASONING,
            Some(false) => unreachable!("handled above"),
            None => DEFAULT_REASONING,
        }),
    )
}

pub fn prepare_upstream_reasoning(body: &mut Value, advertised_support: Option<bool>) {
    if advertised_support == Some(false) {
        if let Some(body) = body.as_object_mut() {
            body.remove("reasoning");
        }
        return;
    }
    if body.pointer("/reasoning/effort").and_then(Value::as_str) == Some("ultra") {
        body["reasoning"]["effort"] = Value::String("max".to_owned());
    }
}

pub fn anthropic_thinking_kind(model: &str) -> Option<AnthropicThinkingKind> {
    detect_model_reasoning(model).and_then(|capabilities| capabilities.anthropic_thinking)
}

pub fn anthropic_thinking_kind_with_advertised(
    model: &str,
    advertised_support: Option<bool>,
) -> Option<AnthropicThinkingKind> {
    resolve_model_reasoning(model, advertised_support)
        .and_then(|capabilities| capabilities.anthropic_thinking)
}

fn model_or_provider_qualified(model: &str, canonical: &str) -> bool {
    model == canonical
        || model
            .strip_prefix(canonical)
            .is_some_and(|suffix| suffix.starts_with('-'))
}

fn uses_adaptive_anthropic_thinking(model: &str) -> bool {
    [
        "fable",
        "mythos",
        "sonnet 5",
        "sonnet-5",
        "sonnet_5",
        "sonnet 4.6",
        "sonnet-4-6",
        "sonnet_4_6",
        "opus 5",
        "opus-5",
        "opus_5",
        "opus 4.8",
        "opus-4-8",
        "opus_4_8",
        "opus 4.7",
        "opus-4-7",
        "opus_4_7",
        "opus 4.6",
        "opus-4-6",
        "opus_4_6",
    ]
    .iter()
    .any(|needle| model.contains(needle))
}

fn uses_manual_anthropic_thinking(model: &str) -> bool {
    [
        "sonnet 3.7",
        "sonnet-3-7",
        "sonnet_3_7",
        "sonnet 4",
        "sonnet-4",
        "sonnet_4",
        "opus 4",
        "opus-4",
        "opus_4",
        "haiku 4.5",
        "haiku-4-5",
        "haiku_4_5",
    ]
    .iter()
    .any(|needle| model.contains(needle))
}
