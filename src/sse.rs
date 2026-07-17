use bytes::Bytes;
use serde::Serialize;

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
            let Some(first_break_len) = line_break_len(&self.buffer, cursor) else {
                cursor += 1;
                continue;
            };
            let second_break_start = cursor + first_break_len;
            let Some(second_break_len) = line_break_len(&self.buffer, second_break_start) else {
                cursor += first_break_len;
                continue;
            };
            if let Some(event) = parse_event(&self.buffer[event_start..cursor]) {
                events.push(event);
            }
            event_start = second_break_start + second_break_len;
            cursor = event_start;
        }
        if event_start > 0 {
            self.buffer.drain(..event_start);
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
    let text = String::from_utf8_lossy(raw);
    let mut event = None;
    let mut data_lines = Vec::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim_start().to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim_start());
        }
    }
    if data_lines.is_empty() {
        return None;
    }
    Some(SseEvent {
        event,
        data: data_lines.join("\n"),
    })
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
}
