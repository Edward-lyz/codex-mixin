//! Pure native Kiro request translation and AWS EventStream validation.
mod events;

pub(crate) use events::{EventDecoder, KiroSse};

use anyhow::{Context, bail, ensure};
use serde_json::{Value, json};

use crate::anthropic::{ContentBlock, MessageRequest};

pub(crate) fn request_payload(
    request: &MessageRequest,
    profile_arn: Option<&str>,
    conversation_id: &str,
) -> anyhow::Result<Value> {
    ensure!(
        request.speed.is_none(),
        "Kiro does not support Anthropic speed selection"
    );
    ensure!(
        request.thinking.is_none(),
        "Kiro thinking configuration requires model-specific support and is not available through this adapter"
    );
    if let Some(choice) = &request.tool_choice {
        ensure!(
            choice.get("type").and_then(Value::as_str) == Some("auto"),
            "Kiro supports automatic tool choice only"
        );
    }
    let mut history = Vec::<Value>::new();
    for message in &request.messages {
        let user = message.role == "user";
        ensure!(
            user || message.role == "assistant",
            "invalid Kiro message role: {}",
            message.role
        );
        let mut text = String::new();
        let mut tool_results = Vec::new();
        let mut tool_uses = Vec::new();
        let mut images = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text { text: part } => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(part);
                }
                ContentBlock::ToolUse { id, name, input } => {
                    ensure!(!user, "Kiro tool calls must be assistant messages");
                    tool_uses.push(json!({"toolUseId":id,"name":name,"input":input}));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } => {
                    ensure!(user, "Kiro tool results must be user messages");
                    let content = match content {
                        Value::String(text) => vec![json!({"text":text})],
                        Value::Array(parts) => parts
                            .iter()
                            .map(|part| {
                                part.get("text")
                                    .and_then(Value::as_str)
                                    .map(|text| json!({"text":text}))
                                    .context("Kiro tool results support text content only")
                            })
                            .collect::<anyhow::Result<Vec<_>>>()?,
                        _ => bail!("Kiro tool result must contain text or text blocks"),
                    };
                    tool_results.push(
                        json!({"toolUseId":tool_use_id,"content":content,"status":"success"}),
                    );
                }
                ContentBlock::Image { source } => {
                    ensure!(user, "Kiro images must be user messages");
                    ensure!(
                        source.get("type").and_then(Value::as_str) == Some("base64"),
                        "Kiro images require base64 sources"
                    );
                    let media = source
                        .get("media_type")
                        .and_then(Value::as_str)
                        .context("Kiro image media_type is missing")?;
                    let format = media
                        .strip_prefix("image/")
                        .filter(|format| matches!(*format, "png" | "jpeg" | "gif" | "webp"))
                        .context("unsupported Kiro image media type")?;
                    let bytes = source
                        .get("data")
                        .and_then(Value::as_str)
                        .context("Kiro image data is missing")?;
                    images.push(json!({"format":format,"source":{"bytes":bytes}}));
                }
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {
                    // Native Kiro history has no signed thinking field.
                }
            }
        }
        let entry = if user {
            let mut message = json!({"content":text,"modelId":request.model,"origin":"AI_EDITOR"});
            if !images.is_empty() {
                message["images"] = json!(images);
            }
            if !tool_results.is_empty() {
                message["userInputMessageContext"] = json!({"toolResults":tool_results});
            }
            json!({"userInputMessage":message})
        } else {
            let mut message = json!({"content":text});
            if !tool_uses.is_empty() {
                message["toolUses"] = json!(tool_uses);
            }
            json!({"assistantResponseMessage":message})
        };
        let key = if user {
            "userInputMessage"
        } else {
            "assistantResponseMessage"
        };
        if let Some(previous) = history.last_mut().and_then(|entry| entry.get_mut(key)) {
            merge_message(previous, &entry[key])?;
        } else {
            history.push(entry);
        }
    }
    let system = request
        .system
        .as_ref()
        .map(|blocks| {
            blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => Ok(text.as_str()),
                    _ => bail!("Kiro system instructions support text blocks only"),
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default()
        .join("\n");
    let first_user = history
        .first_mut()
        .and_then(|entry| entry.get_mut("userInputMessage"))
        .context("Kiro requires an initial user message")?;
    if !system.is_empty() {
        first_user["content"] = json!(format!(
            "{system}\n\n{}",
            first_user["content"].as_str().unwrap_or_default()
        ));
    }
    let current = history
        .pop()
        .context("Kiro requires at least one message")?;
    ensure!(
        current.get("userInputMessage").is_some(),
        "Kiro requires a final user message"
    );
    let mut payload = json!({"conversationState":{
        "chatTriggerType":"MANUAL", "conversationId":conversation_id,
        "currentMessage":current, "history":history,
    }, "inferenceConfig":{"maxTokens":request.max_tokens}});
    let tools = request.tools.iter().map(|tool| {
        let name = tool.get("name").and_then(Value::as_str).context("Kiro tool name is missing")?;
        let schema = tool.get("input_schema").filter(|schema| schema.is_object()).context("Kiro tool input_schema is missing")?;
        Ok(json!({"toolSpecification":{"name":name,"description":tool.get("description").and_then(Value::as_str).unwrap_or(name),"inputSchema":{"json":schema}}}))
    }).collect::<anyhow::Result<Vec<_>>>()?;
    if !tools.is_empty() {
        payload["conversationState"]["currentMessage"]["userInputMessage"]["userInputMessageContext"]
            ["tools"] = json!(tools);
    }
    if let Some(profile) = profile_arn {
        payload["profileArn"] = json!(profile);
    }
    Ok(payload)
}

fn merge_message(previous: &mut Value, next: &Value) -> anyhow::Result<()> {
    previous["content"] = json!(format!(
        "{}\n{}",
        previous["content"].as_str().unwrap_or_default(),
        next["content"].as_str().unwrap_or_default()
    ));
    for key in ["images", "toolUses"] {
        if let Some(parts) = next.get(key).and_then(Value::as_array) {
            let target = previous
                .as_object_mut()
                .context("invalid Kiro message")?
                .entry(key)
                .or_insert_with(|| json!([]));
            target
                .as_array_mut()
                .context("invalid Kiro message parts")?
                .extend(parts.iter().cloned());
        }
    }
    if let Some(parts) = next
        .pointer("/userInputMessageContext/toolResults")
        .and_then(Value::as_array)
    {
        if previous
            .pointer("/userInputMessageContext/toolResults")
            .is_none()
        {
            previous["userInputMessageContext"]["toolResults"] = json!([]);
        }
        previous["userInputMessageContext"]["toolResults"]
            .as_array_mut()
            .context("invalid Kiro tool results")?
            .extend(parts.iter().cloned());
    }
    Ok(())
}
