//! CLI contract of the context console page: settings and statistics JSON.

use std::path::Path;
use std::process::Output;

use codex_mixin::config::ContextSettings;
use codex_mixin::context::{
    ContextEngine,
    types::{SESSION_SEPARATOR, TOOL_COMPRESS, TOOL_STATUS},
};
use serde_json::{Value, json};

mod common;

fn cli(directory: &Path, database: &Path, args: &[&str]) -> Output {
    common::cli(directory)
        .env("CODEX_MIXIN_CONTEXT_DB", database)
        .args(["--no-tui", "--json-errors"])
        .args(args)
        .output()
        .unwrap()
}

fn successful(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn message(role: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{"type": "input_text", "text": text}],
    })
}

fn long_body(label: &str) -> String {
    format!("{label} {}", "word ".repeat(180))
}

#[test]
fn context_cli_reports_settings_and_empty_stats() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("context.db");
    let got = successful(cli(
        directory.path(),
        &database,
        &["context", "config", "get", "--json"],
    ));
    assert_eq!(got["context"]["enabled"], false);
    assert_eq!(got["context"]["growth_threshold_tokens"], 50000);
    assert_eq!(got["database_path"], database.display().to_string());

    let stats = successful(cli(
        directory.path(),
        &database,
        &["context", "stats", "--json"],
    ));
    assert_eq!(stats["enabled"], false);
    assert_eq!(stats["sessions"], json!([]));
    assert_eq!(stats["totals"]["sessions"], 0);
    assert_eq!(stats["totals"]["saved_tokens"], 0);
    assert!(
        !database.exists(),
        "reading statistics must not create the archive"
    );
}

#[test]
fn context_cli_rejects_bad_settings_without_saving() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("context.db");
    let saved = successful(cli(
        directory.path(),
        &database,
        &[
            "context",
            "config",
            "set",
            "--enabled",
            "true",
            "--growth-threshold-tokens",
            "60000",
            "--json",
        ],
    ));
    assert_eq!(saved["context"]["enabled"], true);
    assert_eq!(saved["context"]["growth_threshold_tokens"], 60000);

    let rejected = cli(
        directory.path(),
        &database,
        &[
            "context",
            "config",
            "set",
            "--growth-threshold-tokens",
            "0",
            "--json",
        ],
    );
    assert!(!rejected.status.success());
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.contains("growth_threshold_tokens must be positive"),
        "{stderr}"
    );
    let got = successful(cli(
        directory.path(),
        &database,
        &["context", "config", "get", "--json"],
    ));
    assert_eq!(got["context"]["growth_threshold_tokens"], 60000);
}

#[test]
fn context_cli_reports_compressed_archive() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("context.db");
    let settings = ContextSettings {
        enabled: true,
        protected_messages: 1,
        protected_tokens: 1,
        ..ContextSettings::default()
    };
    let session = format!("codex{SESSION_SEPARATOR}thread-cli");
    let items = vec![
        message("user", "please start the task"),
        message("assistant", &long_body("alpha")),
        message("assistant", &long_body("beta")),
        message("user", "continue"),
    ];
    {
        let engine = ContextEngine::open(settings, &database).unwrap();
        let view = engine.begin_turn(&session, &items, 10_000).unwrap();
        let status = engine
            .execute_tool(&session, view.revision, TOOL_STATUS, &json!({}), 10_000)
            .unwrap();
        let status: Value = serde_json::from_str(&status.content).unwrap();
        let range = &status["compressible"][0];
        let compressed = engine
            .execute_tool(
                &session,
                view.revision,
                TOOL_COMPRESS,
                &json!({
                    "ranges": [{"start": range["start"], "end": range["end"]}],
                    "topic": "work",
                    "summary": "Two long answers about the task.",
                }),
                10_000,
            )
            .unwrap();
        assert!(compressed.changed);
    }

    let stats = successful(cli(
        directory.path(),
        &database,
        &["context", "stats", "--json"],
    ));
    assert_eq!(stats["totals"]["sessions"], 1);
    assert_eq!(stats["totals"]["compressions"], 1);
    assert!(stats["totals"]["saved_tokens"].as_u64().unwrap() > 0);
    let session_row = &stats["sessions"][0];
    assert_eq!(session_row["display"], "codex / thread-cli");
    assert_eq!(session_row["compressions"], 1);
    assert!(session_row["view_tokens"].as_u64().unwrap() > 0);
}
