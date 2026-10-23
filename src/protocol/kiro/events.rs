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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(kind: &str, message_type: &str, payload: Value) -> Vec<u8> {
        let mut headers = Vec::new();
        for (key, value) in [(":message-type", message_type), (":event-type", kind)] {
            headers.push(key.len() as u8);
            headers.extend_from_slice(key.as_bytes());
            headers.push(7);
            headers.extend_from_slice(&(value.len() as u16).to_be_bytes());
            headers.extend_from_slice(value.as_bytes());
        }
        let payload = serde_json::to_vec(&payload).unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(&((16 + headers.len() + payload.len()) as u32).to_be_bytes());
        frame.extend_from_slice(&(headers.len() as u32).to_be_bytes());
        frame.extend_from_slice(&crc32(&frame).to_be_bytes());
        frame.extend(headers);
        frame.extend(payload);
        frame.extend_from_slice(&crc32(&frame).to_be_bytes());
        frame
    }

    #[test]
    fn crc_matches_aws_ieee_polynomial() {
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }

    #[test]
    fn fragmented_and_coalesced_frames_decode() {
        let frames = [
            frame(
                "assistantResponseEvent",
                "event",
                json!({"content":"hello"}),
            ),
            frame(
                "metadataEvent",
                "event",
                json!({"inputTokens":9,"outputTokens":2}),
            ),
        ]
        .concat();
        let mut decoder = EventDecoder::default();
        let mut events = Vec::new();
        for byte in &frames {
            decoder.push(&[*byte]).unwrap();
            while let Some(event) = decoder.next_event().unwrap() {
                events.push(event);
            }
        }
        decoder.finish().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].1["content"], "hello");
        let mut decoder = EventDecoder::default();
        decoder.push(&frames).unwrap();
        assert!(decoder.next_event().unwrap().is_some());
        assert!(decoder.next_event().unwrap().is_some());
        assert!(decoder.next_event().unwrap().is_none());
    }

    #[test]
    fn corrupted_truncated_and_error_frames_fail() {
        let valid = frame(
            "assistantResponseEvent",
            "event",
            json!({"content":"hello"}),
        );
        for offset in [0, 8, valid.len() - 1] {
            let mut invalid = valid.clone();
            invalid[offset] ^= 1;
            let mut decoder = EventDecoder::default();
            decoder.push(&invalid).unwrap();
            assert!(decoder.next_event().is_err());
        }
        let mut decoder = EventDecoder::default();
        decoder.push(&valid[..valid.len() - 1]).unwrap();
        assert!(decoder.next_event().unwrap().is_none());
        assert!(
            decoder
                .finish()
                .unwrap_err()
                .to_string()
                .contains("truncated")
        );
        let mut decoder = EventDecoder::default();
        decoder
            .push(&frame(
                "error",
                "exception",
                json!({"message":"quota exceeded"}),
            ))
            .unwrap();
        assert!(
            decoder
                .next_event()
                .unwrap_err()
                .to_string()
                .contains("quota exceeded")
        );
    }

    #[test]
    fn tool_stream_requires_completion_and_reports_usage() {
        let mut stream = KiroSse::default();
        assert!(stream.finish().is_err());
        stream
            .event("assistantResponseEvent", &json!({"content":"checking"}))
            .unwrap();
        let events = stream
            .event(
                "toolUseEvent",
                &json!({"toolUseId":"t1","name":"read","input":"{\"p\":"}),
            )
            .unwrap();
        assert_eq!(events.len(), 3);
        assert!(stream.finish().is_err());
        stream
            .event(
                "toolUseEvent",
                &json!({"toolUseId":"t1","input":"\"a\"}","stop":true}),
            )
            .unwrap();
        stream
            .event("usageEvent", &json!({"inputTokens":8,"outputTokens":3}))
            .unwrap();
        let events = stream.finish().unwrap();
        let delta = String::from_utf8(events[0].to_vec()).unwrap();
        assert!(delta.contains("tool_use"));
        assert!(delta.contains("\"input_tokens\":8"));
        assert!(delta.contains("\"output_tokens\":3"));
    }
}
