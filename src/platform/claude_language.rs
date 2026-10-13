//! Opt-in Claude Desktop resource localization with full-bundle restore.

use std::path::Path;

pub fn manage_claude_language_pack(app: Option<&Path>, restore: bool) -> anyhow::Result<String> {
    #[cfg(target_os = "macos")]
    {
        macos::manage_claude_language_pack(
            app.unwrap_or_else(|| Path::new("/Applications/Claude.app")),
            restore,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, restore);
        anyhow::bail!("Claude Desktop language packs currently support macOS only")
    }
}

pub fn claude_desktop_has_locale(locale: &str) -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        Some(
            Path::new("/Applications/Claude.app/Contents/Resources/ion-dist/i18n")
                .join(format!("{locale}.json"))
                .is_file(),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = locale;
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use anyhow::{Context, ensure};
    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value};

    const LOCALES: &str = r#"["en-US","de-DE","fr-FR","ko-KR","ja-JP","es-419","es-ES","it-IT","hi-IN","pt-BR","id-ID"]"#;
    const CHINESE_LOCALES: &str = r#"["en-US","de-DE","fr-FR","ko-KR","ja-JP","es-419","es-ES","it-IT","hi-IN","pt-BR","id-ID","zh-Hans","zh-Hant"]"#;
    const ENGLISH_NAME: &str =
        r#"{"en-US":{name:"English (United States)",localName:"English (United States)"}"#;
    const CHINESE_NAMES: &str = r#"{"zh-Hans":{name:"Chinese (Simplified)",localName:"\u7b80\u4f53\u4e2d\u6587"},"zh-Hant":{name:"Chinese (Traditional)",localName:"\u7e41\u9ad4\u4e2d\u6587"},"en-US":{name:"English (United States)",localName:"English (United States)"}"#;

    #[derive(Serialize, Deserialize)]
    struct Manifest {
        app: PathBuf,
        version: String,
    }

    fn run_tool(command: &mut Command, stage: &str) -> anyhow::Result<Vec<u8>> {
        let output = command.output().with_context(|| format!("start {stage}"))?;
        ensure!(
            output.status.success(),
            "{stage}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(output.stdout)
    }

    fn app_version(app: &Path) -> anyhow::Result<String> {
        let plist = app.join("Contents/Info.plist");
        let id = run_tool(
            Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", "Print :CFBundleIdentifier"])
                .arg(&plist),
            "read Claude bundle identity",
        )?;
        ensure!(
            String::from_utf8(id)?.trim() == "com.anthropic.claudefordesktop",
            "language pack target is not Claude Desktop"
        );
        let version = run_tool(
            Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", "Print :CFBundleShortVersionString"])
                .arg(plist),
            "read Claude version",
        )?;
        let version = String::from_utf8(version)?.trim().to_owned();
        ensure!(
            !version.is_empty()
                && version
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')),
            "invalid Claude version"
        );
        Ok(version)
    }

    fn restore_bundle(snapshot: &Path, app: &Path) -> anyhow::Result<()> {
        let parent = app.parent().context("Claude app has no parent directory")?;
        let staging = tempfile::tempdir_in(parent).context("stage Claude bundle restore")?;
        let restored = staging.path().join("Claude.app");
        run_tool(
            Command::new("/usr/bin/ditto").arg(snapshot).arg(&restored),
            "copy original Claude bundle",
        )?;
        run_tool(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(&restored),
            "verify original Claude signature",
        )?;
        let previous = staging.path().join("previous.app");
        if app.exists() {
            fs::rename(app, &previous).context("stage patched Claude bundle")?;
        }
        if let Err(error) = fs::rename(&restored, app) {
            if previous.exists() {
                fs::rename(previous, app).context("roll back Claude bundle replacement")?;
            }
            return Err(error).context("replace Claude bundle with original");
        }
        Ok(())
    }

    fn patch_locales(source: &str) -> (String, usize) {
        let count = source.matches(LOCALES).count() + source.matches(CHINESE_LOCALES).count();
        (
            source
                .replace(LOCALES, CHINESE_LOCALES)
                .replace(ENGLISH_NAME, CHINESE_NAMES),
            count,
        )
    }

    fn merge_catalog(
        base: &Map<String, Value>,
        translated: &Map<String, Value>,
    ) -> (Map<String, Value>, usize) {
        let mut merged = base.clone();
        let mut count = 0;
        for (key, value) in &mut merged {
            if let Some(translation) = translated.get(key) {
                *value = translation.clone();
                count += 1;
            }
        }
        (merged, count)
    }

    fn install_resources(app: &Path) -> anyhow::Result<String> {
        let root = app.join("Contents/Resources/ion-dist");
        let catalog: Map<String, Value> =
            serde_json::from_slice(&fs::read(root.join("i18n/en-US.json"))?)?;
        let mut coverage = Vec::new();
        for (locale, bytes) in [
            (
                "zh-Hans",
                include_bytes!("../../resources/claude-desktop-language/zh-Hans.json.gz")
                    .as_slice(),
            ),
            (
                "zh-Hant",
                include_bytes!("../../resources/claude-desktop-language/zh-Hant.json.gz")
                    .as_slice(),
            ),
        ] {
            let translated: Map<String, Value> =
                serde_json::from_reader(flate2::read::GzDecoder::new(bytes))?;
            ensure!(
                translated.values().all(Value::is_string),
                "invalid Chinese catalog values"
            );
            let (merged, count) = merge_catalog(&catalog, &translated);
            fs::write(
                root.join(format!("i18n/{locale}.json")),
                serde_json::to_vec(&merged)?,
            )?;
            fs::copy(
                root.join("i18n/dynamic/en-US.json"),
                root.join(format!("i18n/dynamic/{locale}.json")),
            )?;
            let overrides: Map<String, Value> =
                serde_json::from_slice(&fs::read(root.join("i18n/en-US.overrides.json"))?)?;
            fs::write(
                root.join(format!("i18n/{locale}.overrides.json")),
                serde_json::to_vec(&merge_catalog(&overrides, &translated).0)?,
            )?;
            coverage.push(format!(
                "{locale}: {count}/{} translated keys",
                catalog.len()
            ));
        }
        let mut count = 0;
        for entry in fs::read_dir(root.join("assets/v1"))? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "js") {
                continue;
            }
            let source = fs::read_to_string(&path)?;
            let (patched, matches) = patch_locales(&source);
            count += matches;
            if patched != source {
                fs::write(path, patched)?;
            }
        }
        // The known local frontend has a browser-match list and a supported list.
        // Stop on layout changes rather than guessing new bundle structure.
        ensure!(
            count >= 2,
            "unsupported Claude frontend: language lists were not found"
        );
        run_tool(
            Command::new("/usr/bin/codesign")
                .args([
                    "--force",
                    "--sign",
                    "-",
                    "--preserve-metadata=entitlements,flags,identifier",
                ])
                .arg(app),
            "sign localized Claude bundle",
        )?;
        run_tool(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(app),
            "verify localized Claude bundle",
        )?;
        Ok(coverage.join("; "))
    }

    /// Install or restore a Chinese pack. Does not contact or manage the gateway.
    pub(super) fn manage_claude_language_pack(app: &Path, restore: bool) -> anyhow::Result<String> {
        ensure!(
            crate::platform::running_desktop_app("Claude").is_none(),
            "fully quit Claude Desktop before changing its language pack"
        );
        let app = std::path::absolute(app)?;
        ensure!(
            app.file_name().is_some_and(|name| name == "Claude.app"),
            "language pack target must be Claude.app"
        );
        let root =
            crate::platform::home_dir_required()?.join(".codex-mixin/claude-language-backups");
        fs::create_dir_all(&root)?;
        crate::platform::set_owner_only_dir_mode(&root)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join("operation.lock"))?;
        fs2::FileExt::lock_exclusive(&lock).context("lock Claude language pack operation")?;
        let current = root.join("current.json");
        if restore {
            let manifest: Manifest = serde_json::from_slice(
                &fs::read(&current).context("read Claude language pack restore manifest")?,
            )?;
            ensure!(
                manifest.app == app && app_version(&app)? == manifest.version,
                "Claude version or app path changed; refusing to replace a newer app with an older backup"
            );
            restore_bundle(&root.join(&manifest.version).join("Claude.app"), &app)?;
            return Ok(format!("restored original Claude {}", manifest.version));
        }
        let locale = crate::platform::system_locale()?;
        ensure!(
            matches!(locale.as_str(), "zh-Hans" | "zh-Hant"),
            "Chinese pack requires a Chinese system display language"
        );
        let version = app_version(&app)?;
        let backup = root.join(&version).join("Claude.app");
        if !backup.exists() {
            let directory = backup
                .parent()
                .context("Claude backup directory is missing")?;
            fs::create_dir_all(directory)?;
            let staging = tempfile::tempdir_in(directory)?;
            let snapshot = staging.path().join("Claude.app");
            run_tool(
                Command::new("/usr/bin/ditto").arg(&app).arg(&snapshot),
                "back up original Claude bundle",
            )?;
            run_tool(
                Command::new("/usr/bin/codesign")
                    .args(["--verify", "--deep", "--strict"])
                    .arg(&snapshot),
                "verify Claude backup",
            )?;
            fs::rename(snapshot, &backup)?;
        }
        let manifest = Manifest {
            app: app.clone(),
            version,
        };
        fs::write(current, serde_json::to_vec_pretty(&manifest)?)?;
        match install_resources(&app) {
            Ok(coverage) => Ok(format!(
                "Claude {} language pack installed ({coverage}); backup: {}",
                manifest.version,
                backup.display()
            )),
            Err(error) => {
                restore_bundle(&backup, &app)
                    .context("restore original Claude after language pack failure")?;
                Err(error).context("Claude language pack failed; original app restored")
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn locale_patch_is_idempotent_and_preserves_other_code() {
            let source = format!("before;{LOCALES};after;{ENGLISH_NAME}");
            let (patched, count) = patch_locales(&source);
            assert_eq!(count, 1);
            assert!(patched.starts_with("before;"));
            assert!(patched.contains(CHINESE_LOCALES));
            assert!(patched.contains(CHINESE_NAMES));
            assert_eq!(patch_locales(&patched), (patched, 1));
        }
        #[test]
        fn catalog_merge_keeps_new_and_unrelated_keys() {
            let base =
                serde_json::from_value(serde_json::json!({"known":"Hello","new":"New control"}))
                    .unwrap();
            let translations =
                serde_json::from_value(serde_json::json!({"known":"Chinese","obsolete":"ignored"}))
                    .unwrap();
            let (merged, count) = merge_catalog(&base, &translations);
            assert_eq!(count, 1);
            assert_eq!(merged["known"], "Chinese");
            assert_eq!(merged["new"], "New control");
            assert!(!merged.contains_key("obsolete"));
        }
    }
}
