use anyhow::{Context, bail, ensure};
use bytes::{Bytes, BytesMut};
use serde_json::{Value, json};

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct EventDecoder {
    buffer: BytesMut,
}

impl EventDecoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        ensure!(
            self.buffer.len() + bytes.len() <= MAX_FRAME_BYTES * 2,
            "Kiro event buffer exceeds 32 MiB"
        );
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn next_event(&mut self) -> anyhow::Result<Option<(String, Value)>> {
        if self.buffer.len() < 12 {
            return Ok(None);
        }
        let total = u32::from_be_bytes(self.buffer[..4].try_into()?) as usize;
        let headers = u32::from_be_bytes(self.buffer[4..8].try_into()?) as usize;
        ensure!(
            (16..=MAX_FRAME_BYTES).contains(&total) && headers <= total - 16,
            "invalid Kiro event frame length"
        );
        ensure!(
            crc32(&self.buffer[..8]) == u32::from_be_bytes(self.buffer[8..12].try_into()?),
            "invalid Kiro event prelude CRC"
        );
        if self.buffer.len() < total {
            return Ok(None);
        }
        let frame = self.buffer.split_to(total);
        ensure!(
            crc32(&frame[..total - 4]) == u32::from_be_bytes(frame[total - 4..].try_into()?),
            "invalid Kiro event message CRC"
        );
        let mut fields = &frame[12..12 + headers];
        let mut event_type = None;
        let mut message_type = None;
        while !fields.is_empty() {
            let name_length = fields[0] as usize;
            ensure!(
                fields.len() >= name_length + 4,
                "truncated Kiro event header"
            );
            let name = std::str::from_utf8(&fields[1..1 + name_length])?;
            ensure!(
                fields[name_length + 1] == 7,
                "unsupported Kiro event header type"
            );
            let length =
                u16::from_be_bytes(fields[name_length + 2..name_length + 4].try_into()?) as usize;
            ensure!(
                fields.len() >= name_length + 4 + length,
                "truncated Kiro event header value"
            );
            let value = std::str::from_utf8(&fields[name_length + 4..name_length + 4 + length])?;
            match name {
                ":event-type" => event_type = Some(value.to_owned()),
                ":message-type" => message_type = Some(value.to_owned()),
                _ => {}
            }
            fields = &fields[name_length + 4 + length..];
        }
        let payload: Value = serde_json::from_slice(&frame[12 + headers..total - 4])
            .context("invalid Kiro event JSON")?;
        ensure!(
            message_type.as_deref() == Some("event"),
            "Kiro upstream error event: {payload}"
        );
        Ok(Some((
            event_type.context("Kiro event type is missing")?,
            payload,
        )))
    }

    pub(crate) fn finish(&self) -> anyhow::Result<()> {
        ensure!(
            self.buffer.is_empty(),
            "Kiro stream ended with a truncated event frame"
        );
        Ok(())
    }
}

// IEEE CRC-32 from the AWS EventStream specification; table keeps per-byte work O(1).
fn crc32(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0; 256];
        let mut index = 0;
        while index < 256 {
            let mut value = index as u32;
            let mut bit = 0;
            while bit < 8 {
                value = if value & 1 == 1 {
                    0xedb88320 ^ (value >> 1)
                } else {
                    value >> 1
                };
                bit += 1;
            }
            table[index] = value;
            index += 1;
        }
        table
    };
    !bytes.iter().fold(!0_u32, |crc, byte| {
        TABLE[((crc ^ u32::from(*byte)) & 255) as usize] ^ (crc >> 8)
    })
}

#[derive(Default)]
pub(crate) struct KiroSse {
    index: usize,
    current_tool: Option<String>,
    block_open: bool,
    had_tool: bool,
    had_content: bool,
    output_tokens: Option<u64>,
    input_tokens: Option<u64>,
}

impl KiroSse {
    pub(crate) fn start(model: &str) -> anyhow::Result<Bytes> {
        encode(
            "message_start",
            json!({"message":{"id":format!("msg_{}",uuid::Uuid::new_v4().simple()),"type":"message","role":"assistant","model":model,"content":[],"usage":{"input_tokens":0,"output_tokens":0}}}),
        )
    }

    pub(crate) fn event(&mut self, kind: &str, payload: &Value) -> anyhow::Result<Vec<Bytes>> {
        let mut events = Vec::new();
        match kind {
            "assistantResponseEvent" => {
                let text = payload
                    .get("content")
                    .and_then(Value::as_str)
                    .context("Kiro assistant event content is missing")?;
                if !text.is_empty() {
                    ensure!(
                        self.current_tool.is_none(),
                        "Kiro sent text before finishing a tool call"
                    );
                    if !self.block_open {
                        events.push(encode(
                            "content_block_start",
                            json!({"index":self.index,"content_block":{"type":"text","text":""}}),
                        )?);
                        self.block_open = true;
                    }
                    self.had_content = true;
                    events.push(encode(
                        "content_block_delta",
                        json!({"index":self.index,"delta":{"type":"text_delta","text":text}}),
                    )?);
                }
            }
            "toolUseEvent" => {
                let id = payload
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .or(self.current_tool.as_deref())
                    .context("Kiro tool id is missing")?
                    .to_owned();
                if self.current_tool.as_deref() != Some(&id) {
                    ensure!(
                        self.current_tool.is_none(),
                        "Kiro interleaved unfinished tool calls"
                    );
                    self.close_block(&mut events)?;
                    let name = payload
                        .get("name")
                        .and_then(Value::as_str)
                        .context("Kiro tool name is missing")?;
                    events.push(encode("content_block_start", json!({"index":self.index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}))?);
                    self.current_tool = Some(id);
                    self.block_open = true;
                    self.had_tool = true;
                    self.had_content = true;
                }
                if let Some(input) = payload.get("input") {
                    let input = match input {
                        Value::String(text) => text.clone(),
                        _ => serde_json::to_string(input)?,
                    };
                    events.push(encode("content_block_delta", json!({"index":self.index,"delta":{"type":"input_json_delta","partial_json":input}}))?);
                }
                if payload.get("stop").and_then(Value::as_bool) == Some(true) {
                    self.current_tool = None;
                    self.close_block(&mut events)?;
                }
            }
            "metadataEvent" | "usageEvent" => {
                let usage = payload.get("usage").unwrap_or(payload);
                self.output_tokens = usage
                    .get("outputTokens")
                    .and_then(Value::as_u64)
                    .or(self.output_tokens);
                self.input_tokens = usage
                    .get("inputTokens")
                    .and_then(Value::as_u64)
                    .or(self.input_tokens);
            }
            kind if kind.ends_with("Exception") || kind.ends_with("Error") => {
                bail!("Kiro {kind}: {payload}")
            }
            _ => {} // References, metering and follow-up suggestions carry no assistant text.
        }
        Ok(events)
    }

    fn close_block(&mut self, events: &mut Vec<Bytes>) -> anyhow::Result<()> {
        if self.block_open {
            events.push(encode("content_block_stop", json!({"index":self.index}))?);
            self.index += 1;
            self.block_open = false;
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> anyhow::Result<Vec<Bytes>> {
        ensure!(
            self.current_tool.is_none(),
            "Kiro stream ended before completing a tool call"
        );
        ensure!(
            self.had_content,
            "Kiro stream ended without assistant content"
        );
        let mut events = Vec::new();
        self.close_block(&mut events)?;
        let mut delta = json!({"delta":{"stop_reason":if self.had_tool { "tool_use" } else { "end_turn" },"stop_sequence":null}});
        if let Some(output) = self.output_tokens {
            delta["usage"] = json!({"output_tokens":output});
            if let Some(input) = self.input_tokens {
                delta["usage"]["input_tokens"] = json!(input);
            }
        }
        events.push(encode("message_delta", delta)?);
        events.push(encode("message_stop", json!({}))?);
        Ok(events)
    }
}

fn encode(kind: &str, mut value: Value) -> anyhow::Result<Bytes> {
    value["type"] = json!(kind);
    Ok(crate::protocol::sse::encode_event(kind, &value)?)
}
