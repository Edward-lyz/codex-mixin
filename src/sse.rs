use bytes::Bytes;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

pub fn encode_event<T: Serialize>(event: &str, data: &T) -> Result<Bytes, serde_json::Error> {
    let data = serde_json::to_string(data)?;
    Ok(Bytes::from(format!("event: {event}\ndata: {data}\n\n")))
}

pub fn encode_raw_event(event: &str, data: &str) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

pub fn drain_events(buffer: &mut Vec<u8>) -> Vec<SseEvent> {
    let mut events = Vec::new();
    while let Some(index) = find_event_boundary(buffer) {
        let raw = buffer.drain(..index).collect::<Vec<_>>();
        let drain_len = if buffer.starts_with(b"\r\n\r\n") {
            4
        } else {
            2
        };
        buffer.drain(..drain_len);
        if let Some(event) = parse_event(&raw) {
            events.push(event);
        }
    }
    events
}

fn find_event_boundary(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .or_else(|| buffer.windows(2).position(|window| window == b"\n\n"))
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
            data_lines.push(value.trim_start().to_owned());
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
}
