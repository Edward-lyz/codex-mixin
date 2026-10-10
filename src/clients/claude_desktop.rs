use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::files::write_owner_only;

pub const PROFILE_ID: &str = "be16e7b5-9352-4cca-a197-32a6e608d930";
const PROFILE_NAME: &str = "Codex Mixin";
const MAX_DESKTOP_MODELS: usize = 200;
const MAX_ROUTE_BYTES: usize = 8192;
const ROUTE_PREFIX: &str = "claude-sonnet-mixin-";

// Encode catalog slugs without a mutable mapping table. Desktop accepts role
// families only; labels carry the real model name, including non-Claude models.
pub fn route_id(model: &str) -> String {
    use std::fmt::Write;
    let mut route = String::with_capacity(ROUTE_PREFIX.len() + model.len() * 2);
    route.push_str(ROUTE_PREFIX);
    for byte in model.bytes() {
        let _ = write!(route, "{byte:02x}");
    }
    route
}

pub(crate) fn route_model(route: &str) -> anyhow::Result<String> {
    let route = route.trim();
    let route = if route.to_ascii_lowercase().ends_with("[1m]") {
        route[..route.len() - 4].trim_end()
    } else {
        route
    };
    let encoded = route
        .strip_prefix(ROUTE_PREFIX)
        .context("unknown Claude Desktop model route")?;
    anyhow::ensure!(
        !encoded.is_empty() && encoded.len() <= MAX_ROUTE_BYTES && encoded.len().is_multiple_of(2),
        "invalid Claude Desktop model route"
    );
    let bytes = encoded
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let pair = std::str::from_utf8(pair)?;
            Ok(u8::from_str_radix(pair, 16)?)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    String::from_utf8(bytes).context("Claude Desktop model route is not UTF-8")
}

fn paths(root: &Path) -> [PathBuf; 5] {
    let library = root.join("Claude-3p/configLibrary");
    [
        root.join("Claude/claude_desktop_config.json"),
        root.join("Claude-3p/claude_desktop_config.json"),
        library.join("_meta.json"),
        library.join(format!("{PROFILE_ID}.json")),
        library.join(format!("{PROFILE_ID}.mixin-backup")),
    ]
}

fn read_object(path: &Path) -> anyhow::Result<Value> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let value: Value =
        serde_json::from_slice(&contents).with_context(|| format!("parse {}", path.display()))?;
    anyhow::ensure!(
        value.is_object(),
        "Claude Desktop configuration must be an object: {}",
        path.display()
    );
    Ok(value)
}

#[derive(Deserialize, Serialize)]
struct Backup {
    previous: Vec<BTreeMap<String, Option<Value>>>,
    installed: Vec<BTreeMap<String, Value>>,
    existed: Vec<bool>,
}

impl Backup {
    fn read(path: &Path) -> anyhow::Result<Self> {
        let backup: Self =
            serde_json::from_slice(&fs::read(path)?).context("parse Claude Desktop backup")?;
        anyhow::ensure!(
            backup.previous.len() == 4 && backup.installed.len() == 4 && backup.existed.len() == 4,
            "invalid Claude Desktop backup"
        );
        Ok(backup)
    }
}

// Save all original bytes before touching any document. On an I/O failure,
// restore every attempted write and report rollback failures with the cause.
fn write_documents(paths: &[PathBuf], documents: &[Option<Value>]) -> anyhow::Result<bool> {
    let originals = paths
        .iter()
        .map(|path| match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut changed = false;
    for (index, (path, document)) in paths.iter().zip(documents).enumerate() {
        let bytes = document
            .as_ref()
            .map(serde_json::to_vec_pretty)
            .transpose()?;
        if bytes == originals[index] {
            continue;
        }
        changed = true;
        let write = match bytes {
            Some(bytes) => write_owner_only(path, &bytes),
            None => fs::remove_file(path).map_err(Into::into),
        };
        if let Err(error) = write {
            let mut failure =
                error.context(format!("write Claude Desktop config {}", path.display()));
            for (path, original) in paths[..=index].iter().zip(&originals) {
                let restore = match original {
                    Some(bytes) => write_owner_only(path, bytes),
                    None if path.exists() => fs::remove_file(path).map_err(Into::into),
                    None => Ok(()),
                };
                if let Err(error) = restore {
                    failure =
                        failure.context(format!("rollback {} failed: {error:#}", path.display()));
                }
            }
            return Err(failure);
        }
    }
    Ok(changed)
}

fn lock_config(backup: &Path) -> anyhow::Result<fs::File> {
    let parent = backup.parent().context("Desktop backup has no parent")?;
    fs::create_dir_all(parent)?;
    let path = backup.with_extension("lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    super::files::set_owner_only(&path)?;
    fs2::FileExt::lock_exclusive(&lock).context("lock Claude Desktop configuration")?;
    Ok(lock)
}

pub fn is_managed(root: &Path) -> anyhow::Result<bool> {
    let paths = paths(root);
    if !paths[4].exists() {
        return Ok(false);
    }
    Backup::read(&paths[4])?;
    Ok(true)
}

pub fn install(root: &Path, base_url: &str, key: &str, models: Value) -> anyhow::Result<bool> {
    apply(root, base_url, key, models, true)
}

pub fn refresh(root: &Path, base_url: &str, key: &str, models: Value) -> anyhow::Result<bool> {
    anyhow::ensure!(is_managed(root)?, "Claude Desktop is not managed");
    apply(root, base_url, key, models, false)
}

fn apply(
    root: &Path,
    base_url: &str,
    key: &str,
    models: Value,
    activate: bool,
) -> anyhow::Result<bool> {
    anyhow::ensure!(!key.is_empty(), "Claude Desktop client key is empty");
    anyhow::ensure!(
        models.as_array().is_some_and(|models| !models.is_empty()),
        "no selected Claude Desktop models"
    );
    let models_list = models
        .as_array()
        .context("Claude Desktop models must be an array")?;
    anyhow::ensure!(
        models_list.len() <= MAX_DESKTOP_MODELS,
        "Claude Desktop supports at most {MAX_DESKTOP_MODELS} selected models"
    );
    for model in models_list {
        route_model(
            model
                .get("name")
                .and_then(Value::as_str)
                .context("Desktop model name is missing")?,
        )?;
    }
    let paths = paths(root);
    let _lock = lock_config(&paths[4])?;
    let mut documents = paths[..4]
        .iter()
        .map(|path| read_object(path))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut backup = if paths[4].exists() {
        Backup::read(&paths[4])?
    } else {
        anyhow::ensure!(
            !paths[3].exists(),
            "Claude Desktop profile ID is already in use: {}",
            paths[3].display()
        );
        Backup {
            previous: vec![BTreeMap::new(); 4],
            installed: vec![BTreeMap::new(); 4],
            existed: paths[..4].iter().map(|path| path.exists()).collect(),
        }
    };
    let patches = [
        json!({"deploymentMode": "3p"}),
        json!({"deploymentMode": "3p"}),
        json!({"appliedId": PROFILE_ID}),
        json!({
            "inferenceProvider": "gateway", "inferenceGatewayBaseUrl": base_url,
            "inferenceGatewayAuthScheme": "bearer", "inferenceGatewayApiKey": key,
            "inferenceModels": models, "disableDeploymentModeChooser": false,
        }),
    ];
    for (index, patch) in patches.iter().enumerate() {
        if !activate && index != 3 {
            continue;
        }
        for (field, value) in patch
            .as_object()
            .context("Desktop patch must be an object")?
        {
            backup.previous[index]
                .entry(field.clone())
                .or_insert_with(|| documents[index].get(field).cloned());
            backup.installed[index].insert(field.clone(), value.clone());
            documents[index][field] = value.clone();
        }
    }
    if activate {
        let entries = documents[2]
            .as_object_mut()
            .context("Desktop metadata must be an object")?
            .entry("entries")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("Claude Desktop metadata entries must be an array")?;
        if !entries
            .iter()
            .any(|entry| entry.get("id").and_then(Value::as_str) == Some(PROFILE_ID))
        {
            entries.push(json!({"id": PROFILE_ID, "name": PROFILE_NAME}));
        }
    }
    // Persist the first-write backup before switching deployment modes, so a
    // process interruption can be repaired by rerunning install or restore.
    let mut write_paths = vec![paths[4].clone()];
    write_paths.extend_from_slice(&paths[..4]);
    let mut writes = vec![Some(serde_json::to_value(&backup)?)];
    writes.extend(documents.into_iter().map(Some));
    write_documents(&write_paths, &writes)
}

pub fn uninstall(root: &Path) -> anyhow::Result<()> {
    let paths = paths(root);
    let _lock = lock_config(&paths[4])?;
    let backup = Backup::read(&paths[4])
        .context("Claude Desktop is not managed or its backup is missing")?;
    let mut documents = paths[..4]
        .iter()
        .map(|path| read_object(path))
        .collect::<anyhow::Result<Vec<_>>>()?;
    for (index, document) in documents.iter_mut().enumerate() {
        let object = document
            .as_object_mut()
            .context("Desktop configuration must be an object")?;
        for (field, installed) in &backup.installed[index] {
            if object.get(field) != Some(installed) {
                continue;
            }
            match backup.previous[index]
                .get(field)
                .context("incomplete Claude Desktop backup")?
            {
                Some(previous) => {
                    object.insert(field.clone(), previous.clone());
                }
                None => {
                    object.remove(field);
                }
            }
        }
    }
    if let Some(entries) = documents[2].get_mut("entries") {
        let entries = entries
            .as_array_mut()
            .context("Claude Desktop metadata entries must be an array")?;
        entries.retain(|entry| entry.get("id").and_then(Value::as_str) != Some(PROFILE_ID));
        if entries.is_empty() && !backup.existed[2] {
            documents[2]
                .as_object_mut()
                .context("Desktop metadata must be an object")?
                .remove("entries");
        }
    }
    let mut writes = documents
        .into_iter()
        .enumerate()
        .map(|(index, document)| {
            if !backup.existed[index] && document.as_object().is_some_and(serde_json::Map::is_empty)
            {
                None
            } else {
                Some(document)
            }
        })
        .collect::<Vec<_>>();
    writes.push(None);
    write_documents(&paths, &writes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_aliases_roundtrip_unicode_and_context_suffix() {
        for model in [
            "GLM-5.3-baidu",
            "\u{6a21}\u{578b} / provider",
            "claude-opus-5-aws",
        ] {
            let alias = route_id(model);
            assert!(alias.starts_with("claude-sonnet-"));
            assert_eq!(route_model(&alias).unwrap(), model);
            assert_eq!(route_model(&format!("{alias} [1M]")).unwrap(), model);
        }
        for alias in [
            "claude-sonnet-mixin-",
            "claude-sonnet-mixin-ff",
            "claude-sonnet-mixin-0",
            "gpt-5",
        ] {
            assert!(route_model(alias).is_err());
        }
    }

    #[test]
    fn install_refresh_restore_preserves_user_changes_and_original_mode() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        write_owner_only(
            &paths[0],
            br#"{"deploymentMode":"1p","mcpServers":{"keep":{}}}"#,
        )
        .unwrap();
        write_owner_only(
            &paths[2],
            br#"{"entries":[{"id":"other","name":"Other"}],"appliedId":"other"}"#,
        )
        .unwrap();
        let models = json!([{"name": route_id("GLM-5.3-baidu"), "labelOverride":"GLM-5.3", "supports1m":false}]);
        assert!(
            install(
                root.path(),
                "http://127.0.0.1:18888/claude-desktop",
                "desktop-key",
                models.clone()
            )
            .unwrap()
        );
        assert!(
            !install(
                root.path(),
                "http://127.0.0.1:18888/claude-desktop",
                "desktop-key",
                models.clone()
            )
            .unwrap()
        );
        install(
            root.path(),
            "http://127.0.0.1:18888/claude-desktop",
            "new-key",
            models,
        )
        .unwrap();
        let mut profile = read_object(&paths[3]).unwrap();
        profile["userAutoMode"] = json!(true);
        write_owner_only(&paths[3], &serde_json::to_vec(&profile).unwrap()).unwrap();
        uninstall(root.path()).unwrap();
        assert_eq!(read_object(&paths[0]).unwrap()["deploymentMode"], "1p");
        assert!(read_object(&paths[0]).unwrap()["mcpServers"]["keep"].is_object());
        assert_eq!(read_object(&paths[2]).unwrap()["appliedId"], "other");
        assert_eq!(
            read_object(&paths[2]).unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            read_object(&paths[3]).unwrap(),
            json!({"userAutoMode":true})
        );
        assert!(!paths[1].exists());
        assert!(!is_managed(root.path()).unwrap());
    }

    #[test]
    fn refresh_does_not_reactivate_a_manually_selected_profile() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let models = json!([{"name": route_id("gpt-5-custom")}]);
        install(root.path(), "http://localhost", "key", models.clone()).unwrap();
        let mut meta = read_object(&paths[2]).unwrap();
        meta["appliedId"] = json!("user-profile");
        write_owner_only(&paths[2], &serde_json::to_vec(&meta).unwrap()).unwrap();
        refresh(root.path(), "http://localhost", "new-key", models).unwrap();
        assert_eq!(read_object(&paths[2]).unwrap()["appliedId"], "user-profile");
        uninstall(root.path()).unwrap();
        assert_eq!(read_object(&paths[2]).unwrap()["appliedId"], "user-profile");
    }

    #[test]
    fn refresh_preserves_language_and_allows_official_login() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let models = json!([{"name": route_id("gpt-5-custom")}]);
        install(root.path(), "http://localhost", "key", models.clone()).unwrap();
        let mut profile = read_object(&paths[3]).unwrap();
        profile["disableDeploymentModeChooser"] = json!(true);
        profile["language"] = json!("ja-JP");
        write_owner_only(&paths[3], &serde_json::to_vec(&profile).unwrap()).unwrap();
        refresh(root.path(), "http://localhost", "key", models).unwrap();
        let refreshed = read_object(&paths[3]).unwrap();
        assert_eq!(refreshed["disableDeploymentModeChooser"], false);
        assert_eq!(refreshed["language"], "ja-JP");
    }

    #[test]
    fn invalid_metadata_does_not_switch_desktop() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        write_owner_only(&paths[2], br#"{"entries":{}}"#).unwrap();
        assert!(
            install(
                root.path(),
                "http://localhost",
                "key",
                json!([{"name": route_id("gpt-5-custom")}])
            )
            .is_err()
        );
        assert!(!paths[0].exists());
        assert!(!paths[4].exists());
    }

    #[cfg(unix)]
    #[test]
    fn failed_write_rolls_back_previous_document() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.json");
        let blocked = root.path().join("blocked");
        fs::write(&first, b"original bytes").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir(&blocked).unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(
            write_documents(
                &[first.clone(), blocked.join("second.json")],
                &[Some(json!({})), Some(json!({}))]
            )
            .is_err()
        );
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(fs::read(first).unwrap(), b"original bytes");
    }
}
