use super::*;

#[derive(Clone, Debug, Default)]
pub(super) struct ThinkingSettings {
    pub(super) thinking: Option<Value>,
    pub(super) output_config: Option<Value>,
}

pub(super) fn convert_thinking(
    model: &str,
    max_tokens: u64,
    reasoning: Option<&Value>,
    config: &GatewayConfig,
) -> Result<ThinkingSettings, GatewayError> {
    let effort = reasoning
        .and_then(|value| value.get("effort"))
        .and_then(Value::as_str);
    if matches!(effort, Some("off" | "none" | "disabled")) {
        return Ok(ThinkingSettings::default());
    }
    match config.thinking_mode {
        ThinkingMode::Off => Ok(ThinkingSettings::default()),
        ThinkingMode::Manual => manual_thinking(max_tokens, effort),
        ThinkingMode::Adaptive => adaptive_thinking(effort),
        ThinkingMode::Auto if model_uses_adaptive_thinking(model) => adaptive_thinking(effort),
        ThinkingMode::Auto if model_uses_manual_thinking(model) => {
            manual_thinking(max_tokens, effort)
        }
        ThinkingMode::Auto => Ok(ThinkingSettings::default()),
    }
}

pub(super) fn adaptive_thinking(effort: Option<&str>) -> Result<ThinkingSettings, GatewayError> {
    Ok(ThinkingSettings {
        thinking: Some(json!({"type": "adaptive", "display": "omitted"})),
        output_config: Some(json!({"effort": adaptive_effort(effort)?})),
    })
}

pub(super) fn manual_thinking(
    max_tokens: u64,
    effort: Option<&str>,
) -> Result<ThinkingSettings, GatewayError> {
    if max_tokens <= 1024 {
        return Err(GatewayError::BadRequest(
            "manual Anthropic thinking requires max_output_tokens greater than 1024".to_owned(),
        ));
    }
    let budget_tokens = manual_budget_tokens(effort)?.min(max_tokens - 1);
    Ok(ThinkingSettings {
        thinking: Some(
            json!({"type": "enabled", "budget_tokens": budget_tokens, "display": "omitted"}),
        ),
        output_config: None,
    })
}

pub(super) fn adaptive_effort(effort: Option<&str>) -> Result<&'static str, GatewayError> {
    match effort.unwrap_or("medium") {
        "minimal" | "low" => Ok("low"),
        "medium" => Ok("medium"),
        "high" => Ok("high"),
        "xhigh" | "exhigh" | "max" => Ok("max"),
        other => Err(GatewayError::BadRequest(format!(
            "unsupported reasoning effort for Anthropic adaptive thinking: {other}"
        ))),
    }
}

pub(super) fn manual_budget_tokens(effort: Option<&str>) -> Result<u64, GatewayError> {
    match effort.unwrap_or("medium") {
        "minimal" | "low" => Ok(1024),
        "medium" => Ok(4096),
        "high" => Ok(8192),
        "xhigh" | "exhigh" | "max" => Ok(16_384),
        other => Err(GatewayError::BadRequest(format!(
            "unsupported reasoning effort for manual Anthropic thinking: {other}"
        ))),
    }
}

pub(super) fn model_uses_adaptive_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    [
        "fable",
        "mythos",
        "sonnet 5",
        "sonnet-5",
        "sonnet_5",
        "sonnet 4.6",
        "sonnet-4-6",
        "sonnet_4_6",
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

pub(super) fn model_uses_manual_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
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
