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

/// Settings the macOS and Windows shells decode from `context config get
/// --json`. Both shells read the object by literal key name, so a rename has
/// to land in the CLI and in every shell together.
const SHARED_SHELL_SETTING_KEYS: [&str; 8] = [
    "enabled",
    "growth_threshold_tokens",
    "pressure_ratio",
    "urgent_ratio",
    "protected_messages",
    "protected_tokens",
    "max_internal_rounds",
    "context_window_tokens",
];

/// Extra settings only the Windows settings page renders.
const WINDOWS_ONLY_SETTING_KEYS: [&str; 2] = ["max_tier", "guidance_only"];

fn assert_source_reads_keys(source: &str, keys: &[&str], shell: &str) {
    for key in keys {
        assert!(
            source.contains(&format!("\"{key}\"")),
            "{shell} no longer decodes {key}"
        );
    }
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
fn context_settings_json_matches_shell_decoders() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("context.db");
    let got = successful(cli(
        directory.path(),
        &database,
        &["context", "config", "get", "--json"],
    ));
    let context = got["context"].as_object().expect("settings object");
    for key in SHARED_SHELL_SETTING_KEYS
        .iter()
        .chain(WINDOWS_ONLY_SETTING_KEYS.iter())
    {
        assert!(context.contains_key(*key), "CLI dropped the {key} setting");
    }
    assert_eq!(
        context.len(),
        SHARED_SHELL_SETTING_KEYS.len() + WINDOWS_ONLY_SETTING_KEYS.len(),
        "unexpected settings keys: {context:?}"
    );

    assert_source_reads_keys(
        include_str!("../macos/ContextSettingsSupport.swift"),
        &SHARED_SHELL_SETTING_KEYS,
        "macos/ContextSettingsSupport.swift",
    );
    assert_source_reads_keys(
        include_str!("../windows/Core/Context.cs"),
        &SHARED_SHELL_SETTING_KEYS,
        "windows/Core/Context.cs",
    );
    assert_source_reads_keys(
        include_str!("../windows/Core/Context.cs"),
        &WINDOWS_ONLY_SETTING_KEYS,
        "windows/Core/Context.cs",
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

#[test]
fn context_stats_use_codex_title_without_changing_session_identity() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("context.db");
    let codex_home = directory.path().join("codex");
    std::fs::create_dir_all(&codex_home).unwrap();
    let titles = rusqlite::Connection::open(codex_home.join("state_5.sqlite")).unwrap();
    titles
        .execute_batch(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, name TEXT);
         INSERT INTO threads VALUES ('thread-title', 'Continue Codex thread task', NULL);",
        )
        .unwrap();
    let sessions: Vec<String> = [
        "thread-title".to_owned(),
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"thread-title").to_string(),
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"thread-id\0thread-title").to_string(),
        uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            b"thread-id\0thread-title\0prompt-cache-key\0thread-title",
        )
        .to_string(),
        "namespaced-subagent".to_owned(),
    ]
    .into_iter()
    .map(|id| format!("Codex{SESSION_SEPARATOR}{id}"))
    .collect();
    let engine = ContextEngine::open(ContextSettings::default(), &database).unwrap();
    for session in &sessions {
        engine
            .begin_turn(session, &[message("user", "continue")], 10_000)
            .unwrap();
    }
    // New rounds retain the source thread even for an opaque routing namespace.
    rusqlite::Connection::open(&database).unwrap().execute(
        "INSERT INTO turns (session_key, at_ms, input_tokens, cached_tokens, output_tokens, codex_thread_id)
         VALUES (?1, 1, 100, 0, 1, 'thread-title')", [&sessions[4]],
    ).unwrap();
    let stats = successful(cli(
        directory.path(),
        &database,
        &["context", "stats", "--json"],
    ));
    assert_eq!(stats["sessions"].as_array().unwrap().len(), sessions.len());
    for row in stats["sessions"].as_array().unwrap() {
        assert!(sessions.iter().any(|session| row["session"] == *session));
        assert_eq!(row["display"], "Continue Codex thread task");
    }

    titles
        .execute("UPDATE threads SET name = 'Renamed task'", [])
        .unwrap();
    let renamed = successful(cli(
        directory.path(),
        &database,
        &["context", "stats", "--json"],
    ));
    for row in renamed["sessions"].as_array().unwrap() {
        assert_eq!(row["display"], "Renamed task");
    }
}
