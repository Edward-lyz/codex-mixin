//! CLI contract of the context console page: settings and statistics JSON.

use std::collections::BTreeSet;
use std::fs;
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
fn context_save_preserves_commit_state_when_gateway_restart_fails() {
    use codex_mixin::config::{
        StoredGatewayConfig, load_stored_config_from_path, save_stored_config_to_path,
    };
    use codex_mixin::provider::custom_provider;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let config_path = root.join("config.json");
    save_stored_config_to_path(
        &config_path,
        &StoredGatewayConfig {
            providers: vec![custom_provider("local", "fixture-key")],
            ..Default::default()
        },
    )
    .unwrap();
    // A directory is not valid runtime metadata. The real restart path must
    // report failure after saving, without touching any user's service.
    fs::create_dir(root.join("runtime.json")).unwrap();
    let result = cli(
        root,
        &root.join("context.db"),
        &["context", "config", "set", "--enabled", "true", "--json"],
    );
    assert!(!result.status.success());
    let errors = String::from_utf8(result.stderr).unwrap();
    let envelope: Value = serde_json::from_str(errors.lines().last().unwrap()).unwrap();
    assert_eq!(envelope["error"]["committed"], true);
    assert_eq!(envelope["error"]["stage"], "context gateway restart");
    assert!(
        load_stored_config_from_path(&config_path)
            .unwrap()
            .unwrap()
            .context
            .enabled
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
    assert!(
        !directory.path().join("runtime.json").exists(),
        "offline settings must not start a gateway"
    );

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
    assert_eq!(session_row["display"], "codex / please start the task");
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

#[test]
fn stats_preview_missing_titles() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("context.db");
    let engine = ContextEngine::open(ContextSettings::default(), &database).unwrap();
    let session = format!("Codex{SESSION_SEPARATOR}opaque-routing-id");
    engine
        .begin_turn(
            &session,
            &[
                message("developer", "Internal tools"),
                message(
                    "user",
                    "# AGENTS.md instructions for workspace\n<INSTRUCTIONS>rules</INSTRUCTIONS>",
                ),
                message(
                    "user",
                    "<environment_context>workspace</environment_context>",
                ),
                message("user", "Fix the session names\nand simplify the interface"),
                message("user", "A later request"),
            ],
            10_000,
        )
        .unwrap();
    let stats = successful(cli(
        directory.path(),
        &database,
        &["context", "stats", "--json"],
    ));
    assert_eq!(
        stats["sessions"][0]["display"],
        "Codex / Fix the session names and simplify the interface"
    );
    assert_eq!(stats["sessions"][0]["session"], session);
}

#[test]
fn stats_client_titles_rename() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("context.db");
    let engine = ContextEngine::open(ContextSettings::default(), &database).unwrap();
    let dsh = directory
        .path()
        .join("home/.dsh/storages/session_projcache/sessions");
    let claude = directory.path().join("home/.claude/projects/workspace");
    std::fs::create_dir_all(&dsh).unwrap();
    std::fs::create_dir_all(&claude).unwrap();
    let dsh_file = dsh.join("session-task.json");
    std::fs::write(
        &dsh_file,
        r#"{"version":7,"record":{"rows":{"title":{"val":"DSH task"}}}}"#,
    )
    .unwrap();
    let claude_file = claude.join("claude-task.jsonl");
    std::fs::write(&claude_file, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"Claude task\"}}\n{\"type\":\"custom-title\",\"customTitle\":\"Renamed Claude task\",\"sessionId\":\"claude-task\"}\n").unwrap();
    for (client, id) in [
        ("DSH", "session-task"),
        ("Claude", "claude-task"),
        ("Codex", "session-task"),
    ] {
        let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, id.as_bytes());
        engine
            .begin_turn(
                &format!("{client}{SESSION_SEPARATOR}{id}"),
                &[message("user", "Original request")],
                10_000,
            )
            .unwrap();
    }
    let stats = successful(cli(
        directory.path(),
        &database,
        &["context", "stats", "--json"],
    ));
    for row in stats["sessions"].as_array().unwrap() {
        let session = row["session"].as_str().unwrap();
        let expected = if session.starts_with("DSH") {
            "DSH task"
        } else if session.starts_with("Claude") {
            "Renamed Claude task"
        } else {
            "Codex / Original request"
        };
        assert_eq!(row["display"], expected);
    }
    std::fs::write(
        &dsh_file,
        r#"{"version":7,"record":{"rows":{"title":{"val":"Renamed DSH task"}}}}"#,
    )
    .unwrap();
    let renamed = successful(cli(
        directory.path(),
        &database,
        &["context", "stats", "--json"],
    ));
    assert!(
        renamed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["display"] == "Renamed DSH task")
    );
}

#[test]
fn stats_keep_previews_when_client_title_stores_are_corrupt() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("context.db");
    let dsh = directory.path().join("home/.dsh/storages");
    let claude = directory.path().join("home/.claude/projects/workspace");
    fs::create_dir_all(&dsh).unwrap();
    fs::create_dir_all(&claude).unwrap();
    // A corrupted client store must not hide the usage rows.
    fs::write(dsh.join("session_projcache.json"), "{not json").unwrap();
    fs::write(claude.join("sessions-index.json"), "{not json").unwrap();
    let engine = ContextEngine::open(ContextSettings::default(), &database).unwrap();
    for client in ["DSH", "Claude"] {
        let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, client.as_bytes());
        engine
            .begin_turn(
                &format!("{client}{SESSION_SEPARATOR}{id}"),
                &[message("user", "Recover this session")],
                10_000,
            )
            .unwrap();
    }
    let stats = successful(cli(
        directory.path(),
        &database,
        &["context", "stats", "--json"],
    ));
    let displays = stats["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["display"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        displays,
        BTreeSet::from([
            "Claude / Recover this session".to_owned(),
            "DSH / Recover this session".to_owned()
        ])
    );
}
