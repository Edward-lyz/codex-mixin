use bytes::Bytes;
use memchr::memchr;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    scan_from: usize,
}

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut event_start = 0;
        let mut cursor = self.scan_from.min(self.buffer.len());
        while cursor < self.buffer.len() {
            let Some(relative_break) = memchr(b'\n', &self.buffer[cursor..]) else {
                break;
            };
            let first_break = cursor + relative_break;
            let second_break_start = first_break + 1;
            let Some(second_break_len) = line_break_len(&self.buffer, second_break_start) else {
                cursor = second_break_start;
                continue;
            };
            if let Some(event) = parse_event(&self.buffer[event_start..first_break]) {
                events.push(event);
            }
            let event_end = second_break_start + second_break_len;
            event_start = event_end;
            cursor = event_end;
        }
        if event_start > 0 {
            self.buffer.copy_within(event_start.., 0);
            self.buffer.truncate(self.buffer.len() - event_start);
        }
        self.scan_from = self.buffer.len().saturating_sub(3);
        events
    }

    pub fn remaining(&self) -> &[u8] {
        &self.buffer
    }
}

pub fn encode_event<T: Serialize>(event: &str, data: &T) -> Result<Bytes, serde_json::Error> {
    let mut encoded = Vec::with_capacity(event.len() + 128);
    encoded.extend_from_slice(b"event: ");
    encoded.extend_from_slice(event.as_bytes());
    encoded.extend_from_slice(b"\ndata: ");
    serde_json::to_writer(&mut encoded, data)?;
    encoded.extend_from_slice(b"\n\n");
    Ok(Bytes::from(encoded))
}

pub fn encode_raw_event(event: &str, data: &str) -> Bytes {
    let mut encoded = Vec::with_capacity(event.len() + data.len() + 16);
    encoded.extend_from_slice(b"event: ");
    encoded.extend_from_slice(event.as_bytes());
    encoded.extend_from_slice(b"\ndata: ");
    encoded.extend_from_slice(data.as_bytes());
    encoded.extend_from_slice(b"\n\n");
    Bytes::from(encoded)
}

pub(crate) fn event_contains_response_metadata(event: &str) -> bool {
    matches!(
        event,
        "response.created"
            | "response.queued"
            | "response.in_progress"
            | "response.completed"
            | "response.failed"
            | "response.incomplete"
    )
}

pub(crate) fn response_failed_payload(
    response_id: Option<String>,
    model: Option<&str>,
    message: impl Into<String>,
    error_type: &str,
) -> Value {
    let error = json!({"message": message.into(), "type": error_type});
    let mut response = json!({
        "id": response_id.unwrap_or_else(|| format!("resp_{}", uuid::Uuid::new_v4().simple())),
        "object": "response",
        "status": "failed",
        "error": error,
        "output": []
    });
    if let Some(model) = model {
        response["model"] = Value::String(model.to_owned());
    }
    json!({"type": "response.failed", "response": response, "error": error})
}

pub fn drain_events(buffer: &mut Vec<u8>) -> Vec<SseEvent> {
    let mut decoder = SseDecoder {
        buffer: std::mem::take(buffer),
        scan_from: 0,
    };
    let events = decoder.push(&[]);
    *buffer = decoder.buffer;
    events
}

fn line_break_len(buffer: &[u8], index: usize) -> Option<usize> {
    match buffer.get(index) {
        Some(b'\n') => Some(1),
        Some(b'\r') if buffer.get(index + 1) == Some(&b'\n') => Some(2),
        _ => None,
    }
}

fn parse_event(raw: &[u8]) -> Option<SseEvent> {
    let mut event = None;
    let mut data = String::new();
    let mut has_data = false;
    let mut rest = raw;
    loop {
        let (line, remainder) = match memchr(b'\n', rest) {
            Some(offset) => (&rest[..offset], &rest[offset + 1..]),
            None => (rest, &[][..]),
        };
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() || line.first() == Some(&b':') {
            if remainder.is_empty() {
                break;
            }
            rest = remainder;
            continue;
        }
        if let Some(value) = line.strip_prefix(b"event:") {
            event = Some(ascii_trim_start(value).to_owned());
        } else if let Some(value) = line.strip_prefix(b"data:") {
            if has_data {
                data.push('\n');
            }
            data.push_str(ascii_trim_start(value));
            has_data = true;
        }
        if remainder.is_empty() {
            break;
        }
        rest = remainder;
    }
    if !has_data {
        return None;
    }
    Some(SseEvent { event, data })
}

fn ascii_trim_start(value: &[u8]) -> &str {
    let mut value = value;
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    std::str::from_utf8(value).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chunked_sse_events() {
        let mut buffer = b"event: ping\ndata: {\"type\":\"ping\"}\n\nevent: content_block_delta\ndata: {\"x\":1}".to_vec();
        let events = drain_events(&mut buffer);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("ping"));
        buffer.extend_from_slice(b"\n\n");
        let events = drain_events(&mut buffer);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("content_block_delta"));
    }

    #[test]
    fn decoder_handles_mixed_line_endings_and_keeps_partial_events() {
        let mut decoder = SseDecoder::default();
        let events = decoder.push(b"event: one\r\ndata: 1\r\n\nevent: two\ndata: 2\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("one"));
        assert_eq!(events[0].data, "1");
        assert!(!decoder.remaining().is_empty());

        let events = decoder.push(b"\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("two"));
        assert_eq!(events[0].data, "2");
        assert!(decoder.remaining().is_empty());
    }

    #[test]
    fn preserves_empty_and_multiline_data_fields() {
        let mut decoder = SseDecoder::default();
        let events = decoder.push(b"event: empty\ndata:\n\nevent: multi\ndata: one\ndata: two\n\n");
        assert_eq!(events[0].data, "");
        assert_eq!(events[1].data, "one\ntwo");
    }

    #[test]
    fn identifies_only_events_that_carry_response_metadata() {
        assert!(event_contains_response_metadata("response.created"));
        assert!(event_contains_response_metadata("response.completed"));
        assert!(!event_contains_response_metadata(
            "response.output_text.delta"
        ));
        assert!(!event_contains_response_metadata(
            "response.output_item.done"
        ));
    }

    #[test]
    fn builds_consistent_failed_response_payloads() {
        let payload = response_failed_payload(
            Some("resp_test".to_owned()),
            Some("model-test"),
            "failed",
            "server_error",
        );
        assert_eq!(payload["type"], "response.failed");
        assert_eq!(payload["response"]["id"], "resp_test");
        assert_eq!(payload["response"]["model"], "model-test");
        assert_eq!(payload["response"]["error"]["message"], "failed");
        assert_eq!(payload["error"]["type"], "server_error");
    }
}
