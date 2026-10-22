//! Read-only display metadata. Never load rollouts or retain a title cache:
//! desktop renames must be visible on the next statistics query.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

pub(crate) fn visit_titles(visit: impl FnMut(String, String)) {
    let home = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            crate::platform::home_dir_required()
                .ok()
                .map(|home| home.join(".codex"))
        });
    let Some(home) = home else { return };
    let path = home.join("state_5.sqlite");
    if !path.exists() {
        return;
    }
    // Titles are optional presentation metadata; failure must not hide usage.
    if let Err(error) = read_titles(&path, visit) {
        tracing::warn!(path = %path.display(), error = %error, "Codex titles unavailable; using session identifiers");
    }
}

fn read_titles(path: &Path, mut visit: impl FnMut(String, String)) -> anyhow::Result<()> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_millis(100))?;
    connection.pragma_update(None, "cache_size", -128)?;
    let columns = connection
        .prepare("PRAGMA table_info(threads)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if columns.is_empty() {
        return Ok(());
    }
    // Older Codex builds have title only; current builds store manual renames
    // separately in name. Both schemas are still in use.
    let sql = if columns.iter().any(|column| column == "name") {
        "SELECT id, COALESCE(NULLIF(TRIM(name), ''), title) FROM threads"
    } else {
        "SELECT id, title FROM threads"
    };
    let mut statement = connection.prepare(sql)?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let title: Option<String> = row.get(1)?;
        if let Some(title) = title.filter(|title| !title.trim().is_empty()) {
            visit(row.get(0)?, title);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_legacy_titles_without_creating_or_writing_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state_5.sqlite");
        assert!(read_titles(&path, |_, _| {}).is_err());
        assert!(!path.exists());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (id TEXT, title TEXT);
            INSERT INTO threads VALUES ('a', 'Task'), ('b', ''), ('c', NULL);",
            )
            .unwrap();
        let mut titles = Vec::new();
        read_titles(&path, |id, title| titles.push((id, title))).unwrap();
        assert_eq!(titles, [("a".to_owned(), "Task".to_owned())]);
        assert_eq!(connection.total_changes(), 3);
    }
}
