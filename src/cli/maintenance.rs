use std::path::PathBuf;

use codex_mixin::history::migrate_history_to_mixin_provider;

use super::codex::codex_home_path;

pub(super) fn migrate_history(codex_home: Option<PathBuf>) -> anyhow::Result<()> {
    let codex_home = codex_home.unwrap_or_else(codex_home_path);
    let outcome = migrate_history_to_mixin_provider(&codex_home)?;
    println!(
        "history jsonl files changed: {}",
        outcome.jsonl_files_changed
    );
    println!(
        "history jsonl lines changed: {}",
        outcome.jsonl_lines_changed
    );
    println!(
        "history sqlite rows changed: {}",
        outcome.sqlite_rows_changed
    );
    if let Some(backup_root) = outcome.backup_root {
        println!("history backup: {}", backup_root.display());
    } else {
        println!("history backup: <none; no changes>");
    }
    Ok(())
}
