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

#[cfg(test)]
mod tests {
    use super::*;

    fn efforts(capabilities: ModelReasoningCapabilities) -> Vec<&'static str> {
        capabilities
            .supported_levels
            .iter()
            .map(|level| level.effort)
            .collect()
    }

    #[test]
    fn detects_provider_qualified_gpt_profiles() {
        let sol = detect_model_reasoning("gpt-5.6-sol-baidu-oneapi").unwrap();
        assert_eq!(sol.default_effort, "low");
        assert_eq!(
            efforts(sol),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(sol.multi_agent_version, Some("v2"));

        let terra = detect_model_reasoning("GPT-5.6-Terra-custom").unwrap();
        assert_eq!(terra.default_effort, "medium");
        assert_eq!(terra.multi_agent_version, Some("v2"));

        let luna = detect_model_reasoning("gpt-5.6-luna-baidu-oneapi").unwrap();
        assert_eq!(
            efforts(luna),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(luna.multi_agent_version, Some("v2"));

        let gpt_5_5 = detect_model_reasoning("gpt-5.5-baidu-oneapi").unwrap();
        assert_eq!(
            efforts(gpt_5_5),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(gpt_5_5.multi_agent_version, Some("v2"));

        let gpt_5_4 = detect_model_reasoning("gpt-5.4-baidu-oneapi").unwrap();
        assert_eq!(
            efforts(gpt_5_4),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(gpt_5_4.multi_agent_version, Some("v2"));
    }

    #[test]
    fn detects_adaptive_claude_with_ultra() {
        for model in [
            "Claude Opus 4.6",
            "Claude Opus 4.7-baidu-oneapi",
            "Opus 4.8",
            "Opus 5",
            "Claude Sonnet 5",
            "Fable 5",
        ] {
            let capabilities = detect_model_reasoning(model).unwrap();
            assert_eq!(
                capabilities.anthropic_thinking,
                Some(AnthropicThinkingKind::Adaptive),
                "{model}"
            );
            assert_eq!(
                efforts(capabilities),
                ["none", "low", "medium", "high", "xhigh", "max", "ultra"],
                "{model}"
            );
            assert_eq!(capabilities.multi_agent_version, Some("v2"), "{model}");
        }
    }

    #[test]
    fn keeps_older_claude_on_manual_thinking() {
        let capabilities = detect_model_reasoning("Claude Haiku 4.5").unwrap();
        assert_eq!(
            capabilities.anthropic_thinking,
            Some(AnthropicThinkingKind::Manual)
        );
        assert_eq!(
            efforts(capabilities),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(capabilities.multi_agent_version, Some("v2"));
    }

    #[test]
    fn uses_default_profile_for_other_models() {
        assert_eq!(detect_model_reasoning("DeepSeek-V4-Flash"), None);
        for advertised_support in [None, Some(true)] {
            let deepseek =
                resolve_model_reasoning("DeepSeek-V4-Flash", advertised_support).unwrap();
            assert_eq!(
                efforts(deepseek),
                ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
            );
            assert_eq!(deepseek.multi_agent_version, Some("v2"));
            assert_eq!(
                deepseek.anthropic_thinking,
                advertised_support.map(|_| AnthropicThinkingKind::Adaptive)
            );
        }
        let without_thinking = resolve_model_reasoning("DeepSeek-V4-Flash", Some(false)).unwrap();
        assert_eq!(efforts(without_thinking), ["none", "ultra"]);
        assert_eq!(without_thinking.default_effort, "none");
        assert_eq!(without_thinking.multi_agent_version, Some("v2"));
    }

    #[test]
    fn advertised_support_does_not_override_known_manual_claude() {
        assert_eq!(
            anthropic_thinking_kind_with_advertised("Claude Haiku 4.5", Some(true)),
            Some(AnthropicThinkingKind::Manual)
        );
    }

    #[test]
    fn prepares_ultra_for_upstream_without_leaking_the_client_only_effort() {
        let mut supported = serde_json::json!({"reasoning":{"effort":"ultra","summary":"auto"}});
        prepare_upstream_reasoning(&mut supported, Some(true));
        assert_eq!(supported["reasoning"]["effort"], "max");
        assert_eq!(supported["reasoning"]["summary"], "auto");

        let mut unsupported = serde_json::json!({"reasoning":{"effort":"ultra"}});
        prepare_upstream_reasoning(&mut unsupported, Some(false));
        assert!(unsupported.get("reasoning").is_none());
    }
}
