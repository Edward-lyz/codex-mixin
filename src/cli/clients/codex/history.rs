use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use codex_mixin::CODEX_MIXIN_PROVIDER;
use rusqlite::Connection;
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct HistoryMigrationOutcome {
    pub backup_root: Option<PathBuf>,
    pub jsonl_files_changed: usize,
    pub jsonl_lines_changed: usize,
    pub sqlite_rows_changed: usize,
    /// Items left unchanged because they could not be read, backed up or
    /// rewritten; the remaining history is still processed.
    pub failures: Vec<String>,
}

pub fn migrate_history_to_mixin_provider(
    codex_home: &Path,
) -> anyhow::Result<HistoryMigrationOutcome> {
    migrate_history_to_provider(codex_home, CODEX_MIXIN_PROVIDER)
}

pub fn migrate_history_to_provider(
    codex_home: &Path,
    target_provider: &str,
) -> anyhow::Result<HistoryMigrationOutcome> {
    migrate_history_provider(
        codex_home,
        None,
        target_provider,
        &format!("unify-{target_provider}"),
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
    for dir in ["sessions", "archived_sessions"] {
        let dir = codex_home.join(dir);
        if let Err(error) = collect_jsonl_files(&dir, &mut session_files) {
            outcome
                .failures
                .push(format!("scan {}: {error:#}", dir.display()));
        }
    }
    for path in session_files {
        match rewrite_jsonl_session_meta(
            &path,
            codex_home,
            &backup_root,
            source_provider,
            target_provider,
        ) {
            Ok(0) => {}
            Ok(changed_lines) => {
                outcome.jsonl_files_changed += 1;
                outcome.jsonl_lines_changed += changed_lines;
            }
            Err(error) => outcome
                .failures
                .push(format!("{}: {error:#}", path.display())),
        }
    }

    for (db_path, table) in [
        (codex_home.join("state_5.sqlite"), "threads"),
        (
            codex_home.join("sqlite").join("codex-dev.db"),
            "local_thread_catalog",
        ),
    ] {
        match migrate_sqlite_table(
            codex_home,
            &backup_root,
            &db_path,
            table,
            source_provider,
            target_provider,
        ) {
            Ok(rows) => outcome.sqlite_rows_changed += rows,
            Err(error) => outcome
                .failures
                .push(format!("{} ({table}): {error:#}", db_path.display())),
        }
    }

    if outcome.jsonl_files_changed == 0 && outcome.sqlite_rows_changed == 0 {
        outcome.backup_root = None;
        if backup_root.exists()
            && let Err(error) = fs::remove_dir_all(&backup_root)
        {
            outcome
                .failures
                .push(format!("remove {}: {error}", backup_root.display()));
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
    codex_mixin::platform::write_atomic_if_changed(path, rewritten.as_bytes())?;
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
    if !db_path.exists() {
        return Ok(0);
    }
    let connection = Connection::open(db_path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    // `table` is one of the fixed names above; provider values are bound.
    let predicate = "model_provider IS NOT NULL AND model_provider <> ?1 AND (?2 IS NULL OR model_provider = ?2)";
    let count: i64 = connection.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE {predicate}"),
        rusqlite::params![target_provider, source_provider],
        |row| row.get(0),
    )?;
    if count == 0 {
        return Ok(0);
    }
    // VACUUM INTO reads through SQLite, so the copy includes committed WAL
    // frames that a plain copy of the main database file would miss.
    let backup_path = backup_root.join(db_path.strip_prefix(codex_home).unwrap_or(db_path));
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let backup_path = backup_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("SQLite backup path is not UTF-8"))?;
    connection.execute("VACUUM INTO ?1", [backup_path])?;
    Ok(connection.execute(
        &format!("UPDATE {table} SET model_provider = ?1 WHERE {predicate}"),
        rusqlite::params![target_provider, source_provider],
    )?)
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
