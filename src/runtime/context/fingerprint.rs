use std::io::{self, Write};

use anyhow::{Context, Result};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::{Value, json};

pub(super) fn fingerprint(item: &Value) -> Result<String> {
    let mut serializer = serde_json::Serializer::new(DigestWriter {
        context: ring::digest::Context::new(&ring::digest::SHA256),
    });
    FingerprintValue(item)
        .serialize(&mut serializer)
        .context("serialize context fingerprint")?;
    let digest = serializer.into_inner().context.finish();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(64);
    for byte in digest.as_ref() {
        hex.push(char::from(HEX[usize::from(*byte >> 4)]));
        hex.push(char::from(HEX[usize::from(*byte & 0x0f)]));
    }
    Ok(hex)
}

struct DigestWriter {
    context: ring::digest::Context,
}

impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.context.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FingerprintValue<'a>(&'a Value);

impl Serialize for FingerprintValue<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let Some(object) = self.0.as_object() else {
            return self.0.serialize(serializer);
        };
        let omit_reasoning_content = object.get("type").and_then(Value::as_str)
            == Some("reasoning")
            && object.get("content").is_some_and(Value::is_null);
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in object {
            if key == "id" || key == "status" || (omit_reasoning_content && key == "content") {
                continue;
            }
            if key == "internal_chat_message_metadata_passthrough" {
                // Codex stamps history after the raw response was received. Its
                // delta comparison ignores those stamps, but treats late tool
                // results as edits. Keep the same result-bearing projection.
                let results: Vec<Value> = value
                    .get("executed_tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .filter_map(|(index, call)| {
                        let result = call.get("tool_result_metadata").filter(|v| !v.is_null())?;
                        Some(json!([
                            index,
                            call.get("name"),
                            call.get("arguments"),
                            result
                        ]))
                    })
                    .collect();
                if !results.is_empty() {
                    map.serialize_entry(key, &results)?;
                }
                continue;
            }
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}
