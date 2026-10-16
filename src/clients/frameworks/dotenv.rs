//! Dotenv edits retain non-managed line contents; CRLF is normalized to LF.
//! Values are raw right-hand sides, not shell input. Credentials use JSON quoting.
use anyhow::Context;
use serde_json::{Map, Value, json};

const LINES: &str = "$lines";
const NEWLINE: &str = "$newline";

pub(super) fn parse(text: &str) -> anyhow::Result<Value> {
    let mut values = Map::new();
    let mut layout = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            layout.push(json!(line));
            continue;
        }
        let assignment = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let (name, rhs) = assignment
            .split_once('=')
            .context("dotenv line must be an assignment or comment")?;
        let name = name.trim();
        anyhow::ensure!(
            !name.is_empty()
                && name.bytes().enumerate().all(|(i, b)| b == b'_'
                    || b.is_ascii_alphabetic()
                    || i > 0 && b.is_ascii_digit()),
            "invalid dotenv variable name"
        );
        // Multiline shell/dotenv quoting needs a dedicated parser. Refuse rather
        // than rewrite a partial assignment or reinterpret shell expressions.
        let value = rhs.trim_start();
        if let Some(quote) = value.chars().next().filter(|c| *c == '\'' || *c == '"') {
            let mut escaped = false;
            let closed = value[1..].chars().any(|character| {
                if escaped {
                    escaped = false;
                    return false;
                }
                if character == '\\' {
                    escaped = true;
                    return false;
                }
                character == quote
            });
            anyhow::ensure!(closed, "multiline dotenv values are not supported");
        }
        anyhow::ensure!(
            !values.contains_key(name),
            "duplicate dotenv variable {name}"
        );
        values.insert(name.to_owned(), json!(rhs));
        let prefix = &line[..line.len() - rhs.len()];
        layout.push(json!([name, prefix]));
    }
    values.insert(LINES.to_owned(), json!(layout));
    values.insert(NEWLINE.to_owned(), json!(text.ends_with('\n')));
    Ok(Value::Object(values))
}

pub(super) fn encode(value: &Value) -> anyhow::Result<String> {
    let object = value.as_object().context("dotenv root must be an object")?;
    let mut remaining = object.clone();
    remaining.remove(LINES);
    remaining.remove(NEWLINE);
    let mut lines = Vec::new();
    for line in object
        .get(LINES)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(text) = line.as_str() {
            lines.push(text.to_owned());
            continue;
        }
        let name = line[0].as_str().context("invalid dotenv layout key")?;
        let prefix = line[1].as_str().context("invalid dotenv layout prefix")?;
        if let Some(rhs) = remaining.remove(name) {
            lines.push(format!(
                "{prefix}{}",
                rhs.as_str().context("dotenv value must be raw text")?
            ));
        }
    }
    for (name, rhs) in remaining {
        lines.push(format!(
            "{name}={}",
            rhs.as_str().context("dotenv value must be raw text")?
        ));
    }
    let mut text = lines.join("\n");
    if object.get(NEWLINE).and_then(Value::as_bool).unwrap_or(true) && !text.is_empty() {
        text.push('\n');
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_unmanaged_lines_and_rejects_duplicates() {
        let original = "# local settings\nexport OTHER = 'keep # literal'\nEMPTY=\n";
        let mut doc = parse(original).unwrap();
        doc["CODEX_MIXIN_REASONIX_API_KEY"] = json!("\"fixture-key\"");
        let text = encode(&doc).unwrap();
        assert!(text.starts_with(original));
        assert!(text.ends_with("CODEX_MIXIN_REASONIX_API_KEY=\"fixture-key\"\n"));
        assert!(parse("KEY=a\nexport KEY=b\n").is_err());
        assert!(parse("KEY=\"first\nsecond\"\n").is_err());
        assert!(parse("KEY=\"first\\\"\nOTHER=second\"\n").is_err());
    }
}
