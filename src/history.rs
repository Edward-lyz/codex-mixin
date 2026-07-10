use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::CODEX_MIXIN_PROVIDER;

#[derive(Clone, Debug, Default)]
pub struct HistoryMigrationOutcome {
    pub backup_root: Option<PathBuf>,
    pub jsonl_files_changed: usize,
    pub jsonl_lines_changed: usize,
    pub sqlite_rows_changed: usize,
}

pub fn migrate_history_to_mixin_provider(
    codex_home: &Path,
) -> anyhow::Result<HistoryMigrationOutcome> {
    migrate_history_provider(codex_home, None, CODEX_MIXIN_PROVIDER, "unify-codex-mixin")
}

pub fn migrate_history_from_mixin_provider(
    codex_home: &Path,
    target_provider: &str,
) -> anyhow::Result<HistoryMigrationOutcome> {
    migrate_history_provider(
        codex_home,
        Some(CODEX_MIXIN_PROVIDER),
        target_provider,
        "restore-from-codex-mixin",
    )
}

fn migrate_history_provider(
    codex_home: &Path,
    source_provider: Option<&str>,
    target_provider: &str,
    backup_label: &str,
) -> anyhow::Result<HistoryMigrationOutcome> {
    if target_provider.is_empty() || source_provider == Some("") {
        anyhow::bail!("model provider cannot be empty");
    }
    let backup_root = codex_home
        .join("backups")
        .join(format!("provider-{backup_label}-{}", unix_timestamp()));
    let mut outcome = HistoryMigrationOutcome {
        backup_root: Some(backup_root.clone()),
        ..Default::default()
    };

    let mut session_files = Vec::new();
    collect_jsonl_files(&codex_home.join("sessions"), &mut session_files)?;
    collect_jsonl_files(&codex_home.join("archived_sessions"), &mut session_files)?;
    for path in session_files {
        let changed_lines = rewrite_jsonl_session_meta(
            &path,
            codex_home,
            &backup_root,
            source_provider,
            target_provider,
        )?;
        if changed_lines > 0 {
            outcome.jsonl_files_changed += 1;
            outcome.jsonl_lines_changed += changed_lines;
        }
    }

    outcome.sqlite_rows_changed += migrate_sqlite_table(
        codex_home,
        &backup_root,
        &codex_home.join("state_5.sqlite"),
        "threads",
        source_provider,
        target_provider,
    )?;
    outcome.sqlite_rows_changed += migrate_sqlite_table(
        codex_home,
        &backup_root,
        &codex_home.join("sqlite").join("codex-dev.db"),
        "local_thread_catalog",
        source_provider,
        target_provider,
    )?;

    if outcome.jsonl_files_changed == 0 && outcome.sqlite_rows_changed == 0 {
        outcome.backup_root = None;
        if backup_root.exists() {
            fs::remove_dir_all(&backup_root)?;
        }
    }
    Ok(outcome)
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn collect_jsonl_files(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    Ok(())
}

fn rewrite_jsonl_session_meta(
    path: &Path,
    codex_home: &Path,
    backup_root: &Path,
    source_provider: Option<&str>,
    target_provider: &str,
) -> anyhow::Result<usize> {
    let content = fs::read_to_string(path)?;
    let mut changed = 0;
    let mut rewritten = String::with_capacity(content.len());
    for segment in content.split_inclusive('\n') {
        let (line, newline) = segment
            .strip_suffix('\n')
            .map(|line| (line, "\n"))
            .unwrap_or((segment, ""));
        if let Some(next_line) = rewrite_session_meta_line(line, source_provider, target_provider) {
            rewritten.push_str(&next_line);
            changed += 1;
        } else {
            rewritten.push_str(line);
        }
        rewritten.push_str(newline);
    }
    if changed == 0 {
        return Ok(0);
    }
    backup_file(path, codex_home, backup_root)?;
    atomic_write(path, rewritten.as_bytes())?;
    Ok(changed)
}

fn rewrite_session_meta_line(
    line: &str,
    source_provider: Option<&str>,
    target_provider: &str,
) -> Option<String> {
    if !line.contains("\"session_meta\"") || !line.contains("\"model_provider\"") {
        return None;
    }
    let mut value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = value.get_mut("payload")?.as_object_mut()?;
    let provider = payload.get("model_provider")?.as_str()?;
    if provider == target_provider
        || source_provider.is_some_and(|source_provider| provider != source_provider)
    {
        return None;
    }
    payload.insert(
        "model_provider".to_owned(),
        Value::String(target_provider.to_owned()),
    );
    serde_json::to_string(&value).ok()
}

fn migrate_sqlite_table(
    codex_home: &Path,
    backup_root: &Path,
    db_path: &Path,
    table: &str,
    source_provider: Option<&str>,
    target_provider: &str,
) -> anyhow::Result<usize> {
    if !db_path.exists() || !sqlite_table_exists(db_path, table)? {
        return Ok(0);
    }
    let escaped_target = target_provider.replace('\'', "''");
    let predicate = source_provider.map_or_else(
        || format!("model_provider IS NOT NULL AND model_provider <> '{escaped_target}'"),
        |source_provider| format!("model_provider = '{}'", source_provider.replace('\'', "''")),
    );
    let count_sql = format!("SELECT COUNT(*) FROM {table} WHERE {predicate};");
    let count = sqlite_scalar_usize(db_path, &count_sql)?;
    if count == 0 {
        return Ok(0);
    }
    backup_file(db_path, codex_home, backup_root)?;
    let update_sql =
        format!("UPDATE {table} SET model_provider = '{escaped_target}' WHERE {predicate};");
    run_sqlite(db_path, &update_sql)?;
    Ok(count)
}

fn sqlite_table_exists(db_path: &Path, table: &str) -> anyhow::Result<bool> {
    let sql = format!("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{table}';");
    Ok(sqlite_scalar_usize(db_path, &sql)? > 0)
}

fn sqlite_scalar_usize(db_path: &Path, sql: &str) -> anyhow::Result<usize> {
    let output = Command::new("sqlite3")
        .args(["-cmd", ".timeout 5000"])
        .arg(db_path)
        .arg(sql)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "sqlite3 failed for {}: {}",
            db_path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = String::from_utf8(output.stdout)?;
    Ok(text.trim().parse()?)
}

fn run_sqlite(db_path: &Path, sql: &str) -> anyhow::Result<()> {
    let output = Command::new("sqlite3")
        .args(["-cmd", ".timeout 5000"])
        .arg(db_path)
        .arg(sql)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "sqlite3 failed for {}: {}",
            db_path.display(),
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

fn backup_file(path: &Path, codex_home: &Path, backup_root: &Path) -> anyhow::Result<()> {
    let relative = path.strip_prefix(codex_home).unwrap_or(path);
    let backup_path = backup_root.join(relative);
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(path, backup_path)?;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_session_meta_provider() {
        let line =
            r#"{"type":"session_meta","payload":{"id":"s1","model_provider":"unknown-provider"}}"#;
        let rewritten = rewrite_session_meta_line(line, None, CODEX_MIXIN_PROVIDER).unwrap();
        assert!(rewritten.contains(r#""model_provider":"codex-mixin""#));
        assert!(rewrite_session_meta_line(&rewritten, None, CODEX_MIXIN_PROVIDER).is_none());
        let restored =
            rewrite_session_meta_line(&rewritten, Some(CODEX_MIXIN_PROVIDER), "custom").unwrap();
        assert!(restored.contains(r#""model_provider":"custom""#));
    }

    #[test]
    fn migrates_legacy_jsonl_and_keeps_backup() {
        let codex_home = tempfile::tempdir().unwrap();
        let session_path = codex_home.path().join("sessions/2026/07/session.jsonl");
        fs::create_dir_all(session_path.parent().unwrap()).unwrap();
        let original = concat!(
            r#"{"type":"session_meta","payload":{"id":"s1","model_provider":"custom"}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
            "\n"
        );
        fs::write(&session_path, original).unwrap();

        let outcome = migrate_history_to_mixin_provider(codex_home.path()).unwrap();

        assert_eq!(outcome.jsonl_files_changed, 1);
        assert_eq!(outcome.jsonl_lines_changed, 1);
        assert!(
            fs::read_to_string(&session_path)
                .unwrap()
                .contains(r#""model_provider":"codex-mixin""#)
        );
        let backup = outcome
            .backup_root
            .unwrap()
            .join("sessions/2026/07/session.jsonl");
        assert_eq!(fs::read_to_string(backup).unwrap(), original);
    }
}
