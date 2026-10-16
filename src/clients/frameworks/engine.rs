//! A durable undo journal written before any configuration mutation.
//! Callers serialize operations and select journals from their private state.
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::clients::files;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) enum Format {
    Json,
    Yaml,
    Toml,
    Env,
}

pub(super) struct Patch {
    pub path: PathBuf,
    pub format: Format,
    pub fields: Vec<(Vec<String>, Value)>,
}

#[derive(Serialize, Deserialize)]
struct Field {
    path: Vec<String>,
    before: Option<Value>,
    after: Value,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    format: Format,
    original: Option<String>,
    applied: String,
    fields: Vec<Field>,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    entries: Vec<Entry>,
}

fn validate_path(path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        path.is_absolute(),
        "configuration path must be absolute: {}",
        path.display()
    );
    for component in path.components() {
        anyhow::ensure!(
            !matches!(component, Component::ParentDir | Component::CurDir),
            "configuration path must be normalized"
        );
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => anyhow::ensure!(
                !meta.file_type().is_symlink(),
                "refusing symbolic link: {}",
                ancestor.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", ancestor.display()));
            }
        }
    }
    Ok(())
}

fn read(path: &Path) -> anyhow::Result<Option<String>> {
    validate_path(path)?;
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

// JSONC is normalized on write; exact original bytes are restored when possible.
pub(super) fn parse_jsonc(text: &str) -> anyhow::Result<Value> {
    let mut bytes = text.as_bytes().to_vec();
    let mut index = 0;
    let mut quoted = false;
    while index < bytes.len() {
        if quoted {
            match bytes[index] {
                b'\\' => index += 1,
                b'"' => quoted = false,
                _ => {}
            }
        } else if bytes[index] == b'"' {
            quoted = true;
        } else if bytes.get(index..index + 2) == Some(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                bytes[index] = b' ';
                index += 1;
            }
            continue;
        } else if bytes.get(index..index + 2) == Some(b"/*") {
            bytes[index] = b' ';
            bytes[index + 1] = b' ';
            index += 2;
            loop {
                anyhow::ensure!(index + 1 < bytes.len(), "unterminated JSONC comment");
                if &bytes[index..index + 2] == b"*/" {
                    bytes[index] = b' ';
                    bytes[index + 1] = b' ';
                    index += 2;
                    break;
                }
                if bytes[index] != b'\n' {
                    bytes[index] = b' ';
                }
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    index = 0;
    quoted = false;
    while index < bytes.len() {
        if quoted {
            match bytes[index] {
                b'\\' => index += 1,
                b'"' => quoted = false,
                _ => {}
            }
        } else if bytes[index] == b'"' {
            quoted = true;
        } else if bytes[index] == b',' {
            let next = bytes[index + 1..].iter().find(|b| !b.is_ascii_whitespace());
            if matches!(next, Some(b']' | b'}')) {
                bytes[index] = b' ';
            }
        }
        index += 1;
    }
    serde_json::from_slice(&bytes).context("parse JSON/JSONC configuration")
}

fn parse(text: Option<&str>, format: Format) -> anyhow::Result<Value> {
    let Some(text) = text else {
        return Ok(json!({}));
    };
    match format {
        Format::Env => super::dotenv::parse(text),
        Format::Json => parse_jsonc(text),
        Format::Yaml => serde_yaml::from_str(text).context("parse YAML configuration"),
        Format::Toml => {
            let document = text
                .parse::<toml_edit::DocumentMut>()
                .context("parse TOML configuration")?;
            toml_item(&toml_edit::Item::Table(document.as_table().clone()))
        }
    }
}

fn encode(value: &Value, format: Format) -> anyhow::Result<String> {
    Ok(match format {
        Format::Env => super::dotenv::encode(value)?,
        Format::Json => serde_json::to_string_pretty(value)? + "\n",
        Format::Yaml => serde_yaml::to_string(value)?,
        Format::Toml => {
            let mut document = toml_edit::DocumentMut::new();
            for (key, value) in value
                .as_object()
                .context("TOML document root must be an object")?
            {
                document[key] = toml_edit::Item::Value(json_to_toml(value)?);
            }
            document.to_string()
        }
    })
}

fn toml_item(item: &toml_edit::Item) -> anyhow::Result<Value> {
    match item {
        toml_edit::Item::None => Ok(Value::Null),
        toml_edit::Item::Value(value) => toml_to_json(value),
        toml_edit::Item::Table(table) => Ok(Value::Object(
            table
                .iter()
                .map(|(k, v)| Ok((k.to_owned(), toml_item(v)?)))
                .collect::<anyhow::Result<_>>()?,
        )),
        toml_edit::Item::ArrayOfTables(tables) => Ok(Value::Array(
            tables
                .iter()
                .map(|t| toml_item(&toml_edit::Item::Table(t.clone())))
                .collect::<anyhow::Result<_>>()?,
        )),
    }
}

fn toml_to_json(value: &toml_edit::Value) -> anyhow::Result<Value> {
    Ok(match value {
        toml_edit::Value::String(v) => json!(v.value()),
        toml_edit::Value::Integer(v) => json!(v.value()),
        toml_edit::Value::Float(v) => {
            anyhow::ensure!(
                v.value().is_finite(),
                "non-finite TOML float cannot be managed"
            );
            json!(v.value())
        }
        toml_edit::Value::Boolean(v) => json!(v.value()),
        toml_edit::Value::Datetime(_) => {
            anyhow::bail!("TOML datetime configuration requires a dedicated adapter")
        }
        toml_edit::Value::Array(a) => json!(
            a.iter()
                .map(toml_to_json)
                .collect::<anyhow::Result<Vec<_>>>()?
        ),
        toml_edit::Value::InlineTable(t) => Value::Object(
            t.iter()
                .map(|(k, v)| Ok((k.to_owned(), toml_to_json(v)?)))
                .collect::<anyhow::Result<_>>()?,
        ),
    })
}

fn json_to_toml(value: &Value) -> anyhow::Result<toml_edit::Value> {
    Ok(match value {
        Value::String(v) => toml_edit::Value::from(v.as_str()),
        Value::Bool(v) => toml_edit::Value::from(*v),
        Value::Number(v) => {
            if let Some(integer) = v.as_i64() {
                toml_edit::Value::from(integer)
            } else {
                toml_edit::Value::from(v.as_f64().context("invalid TOML number")?)
            }
        }
        Value::Array(values) => {
            let mut array = toml_edit::Array::new();
            for value in values {
                array.push(json_to_toml(value)?);
            }
            toml_edit::Value::Array(array)
        }
        Value::Object(values) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in values {
                table.insert(key, json_to_toml(value)?);
            }
            toml_edit::Value::InlineTable(table)
        }
        Value::Null => anyhow::bail!("TOML cannot represent null"),
    })
}

fn get<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(key)?;
    }
    Some(cursor)
}

fn set(value: &mut Value, path: &[String], replacement: Option<Value>) -> anyhow::Result<()> {
    let Some((key, rest)) = path.split_first() else {
        *value = replacement.context("cannot delete document root")?;
        return Ok(());
    };
    let object = value
        .as_object_mut()
        .context("configuration parent must be an object")?;
    if rest.is_empty() {
        if let Some(replacement) = replacement {
            object.insert(key.clone(), replacement);
        } else {
            object.remove(key);
        }
        return Ok(());
    }
    if replacement.is_none() && !object.contains_key(key) {
        return Ok(());
    }
    let child = object.entry(key.clone()).or_insert_with(|| json!({}));
    set(child, rest, replacement)?;
    if child.as_object().is_some_and(serde_json::Map::is_empty) {
        object.remove(key);
    }
    Ok(())
}

pub(super) fn read_config(path: &Path, format: Format) -> anyhow::Result<Value> {
    parse(read(path)?.as_deref(), format).with_context(|| format!("parse {}", path.display()))
}

pub(super) fn apply(manifest: &Path, patches: Vec<Patch>) -> anyhow::Result<()> {
    if read(manifest)?.is_some() {
        anyhow::ensure!(
            is_managed(manifest)?,
            "integration was interrupted; disconnect before retrying"
        );
        return Ok(());
    }
    let mut entries: Vec<Entry> = Vec::new();
    for patch in patches {
        anyhow::ensure!(
            patch.path != manifest && !entries.iter().any(|e| e.path == patch.path),
            "duplicate transaction path"
        );
        let original = read(&patch.path)?;
        let mut value = parse(original.as_deref(), patch.format)?;
        let mut fields: Vec<Field> = Vec::new();
        for (path, after) in patch.fields {
            anyhow::ensure!(
                !fields
                    .iter()
                    .any(|f| f.path.starts_with(&path) || path.starts_with(&f.path)),
                "overlapping configuration fields"
            );
            let before = get(&value, &path).cloned();
            // Provider namespaces are owned only after a journal is created.
            if path.iter().any(|s| s.starts_with("codex-mixin")) {
                anyhow::ensure!(
                    before.is_none(),
                    "codex-mixin configuration already exists without an undo journal"
                );
            }
            fields.push(Field {
                path: path.clone(),
                before,
                after: after.clone(),
            });
            set(&mut value, &path, Some(after))?;
        }
        entries.push(Entry {
            path: patch.path,
            format: patch.format,
            original,
            applied: encode(&value, patch.format)?,
            fields,
        });
    }
    files::ensure_owner_only_dir(manifest.parent().context("undo journal has no parent")?)?;
    files::write_owner_only(manifest, &serde_json::to_vec(&Journal { entries })?)?;
    let journal: Journal = serde_json::from_str(&read(manifest)?.context("undo journal missing")?)?;
    for entry in &journal.entries {
        // Leave the journal on error so Disconnect can recover completed writes.
        files::write_owner_only(&entry.path, entry.applied.as_bytes())?;
    }
    Ok(())
}

pub(super) fn is_managed(manifest: &Path) -> anyhow::Result<bool> {
    let Some(text) = read(manifest)? else {
        return Ok(false);
    };
    let journal: Journal = serde_json::from_str(&text).context("parse framework undo journal")?;
    for entry in journal.entries {
        let value = read_config(&entry.path, entry.format)?;
        anyhow::ensure!(
            entry
                .fields
                .iter()
                .all(|f| get(&value, &f.path) == Some(&f.after)),
            "managed fields changed or installation was interrupted at {}; disconnect to inspect recovery",
            entry.path.display()
        );
    }
    Ok(true)
}

pub(super) fn remove(manifest: &Path) -> anyhow::Result<()> {
    let Some(text) = read(manifest)? else {
        return Ok(());
    };
    let journal: Journal = serde_json::from_str(&text).context("parse framework undo journal")?;
    let mut restores = Vec::new();
    for entry in &journal.entries {
        let current = read(&entry.path)?;
        let restored = if current == entry.original {
            current
        } else if current.as_deref() == Some(&entry.applied) {
            entry.original.clone()
        } else {
            let mut value = parse(current.as_deref(), entry.format)?;
            for field in &entry.fields {
                let actual = get(&value, &field.path);
                anyhow::ensure!(
                    actual == Some(&field.after) || actual == field.before.as_ref(),
                    "refusing to overwrite edited managed field in {}",
                    entry.path.display()
                );
            }
            for field in &entry.fields {
                set(&mut value, &field.path, field.before.clone())?;
            }
            Some(encode(&value, entry.format)?)
        };
        restores.push((&entry.path, restored));
    }
    for (path, contents) in restores {
        match contents {
            Some(text) => files::write_owner_only(path, text.as_bytes())?,
            None if path.exists() => fs::remove_file(path)?,
            None => {}
        }
    }
    fs::remove_file(manifest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_preserves_original_and_unrelated_edits() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("config.json");
        let manifest = root.path().canonicalize().unwrap().join("journal.json");
        let original = "{ // keep this\n\"model\":\"own\",\"other\":1,}\n";
        fs::write(&path, original).unwrap();
        let patch = || Patch {
            path: path.clone(),
            format: Format::Json,
            fields: vec![
                (vec!["model".to_owned()], json!("managed")),
                (
                    vec!["provider".to_owned(), "codex-mixin".to_owned()],
                    json!({"key":"secret"}),
                ),
            ],
        };
        apply(&manifest, vec![patch()]).unwrap();
        assert!(is_managed(&manifest).unwrap());
        remove(&manifest).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        apply(&manifest, vec![patch()]).unwrap();
        let mut value = read_config(&path, Format::Json).unwrap();
        value["other"] = json!(2);
        fs::write(&path, encode(&value, Format::Json).unwrap()).unwrap();
        remove(&manifest).unwrap();
        let value = read_config(&path, Format::Json).unwrap();
        assert_eq!(value["model"], "own");
        assert_eq!(value["other"], 2);
        assert!(value.get("provider").is_none());
    }
    #[test]
    fn drift_fails_before_any_restore() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("config.yaml");
        let manifest = root.path().canonicalize().unwrap().join("journal.json");
        apply(
            &manifest,
            vec![Patch {
                path: path.clone(),
                format: Format::Yaml,
                fields: vec![(vec!["model".to_owned()], json!("managed"))],
            }],
        )
        .unwrap();
        fs::write(&path, "model: user-edit\n").unwrap();
        assert!(is_managed(&manifest).is_err());
        assert!(remove(&manifest).is_err());
        assert!(manifest.exists());
        assert!(fs::read_to_string(path).unwrap().contains("user-edit"));
    }
    #[test]
    fn jsonc_respects_escaped_quotes_and_comment_markers() {
        let value = parse_jsonc(
            "{/*comment*/\"url\":\"https://host/a//b\",\"quote\":\"\\\"/*\",\"array\":[1,],}",
        )
        .unwrap();
        assert_eq!(value["url"], "https://host/a//b");
        assert_eq!(value["array"], json!([1]));
        assert!(parse_jsonc("{/*").is_err());
    }
}
