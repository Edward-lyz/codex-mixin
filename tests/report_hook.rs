//! End-to-end coverage for the DUCX reporting hook model resolution.

mod common;

use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
use codex_mixin::provider::ProviderPreset;
use serde_json::{Value, json};

const SLUG: &str = "DeepSeek-V4.1-Flash-baidu-oneapi";

fn write_reporting_config(root: &Path) {
    let mut provider = ProviderPreset::BaiduOneApi.create("baidu-oneapi", "fixture-key");
    provider.quota_username = Some("fixture-user".to_owned());
    provider.selected_models = vec!["DeepSeek-V4.1-Flash".to_owned()];
    provider.request_policy.baidu_code_report = true;
    provider.request_policy.data_report_client_token = Some("fixture-token".to_owned());
    save_stored_config_to_path(
        &root.join("config.json"),
        &StoredGatewayConfig {
            providers: vec![provider],
            ..Default::default()
        },
    )
    .expect("save reporting config");
}

fn run_hook(root: &Path, event: &str, body: &Value) {
    run_hook_with(root, event, body, |_| {});
}

fn run_hook_with(
    root: &Path,
    event: &str,
    body: &Value,
    configure: impl FnOnce(&mut std::process::Command),
) {
    let mut command = common::cli(root);
    command
        .args(["report-hook", "--event", event])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure(&mut command);
    let mut child = command.spawn().expect("spawn report hook");
    child
        .stdin
        .take()
        .expect("hook stdin")
        .write_all(body.to_string().as_bytes())
        .expect("write hook body");
    let output = child.wait_with_output().expect("wait for report hook");
    assert!(
        output.status.success(),
        "{event} hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn report_log(root: &Path) -> String {
    std::fs::read_to_string(root.join("report-hook.log")).unwrap_or_default()
}

fn persisted_model(root: &Path, session_id: &str) -> Option<String> {
    report_log(root)
        .lines()
        .find(|line| {
            line.contains("DUCX report event persisted")
                && line.contains(&format!("session_id=\"{session_id}\""))
        })
        .and_then(|line| line.split("model=\"").nth(1))
        .map(|rest| rest.split('"').next().unwrap_or_default().to_owned())
}

#[test]
fn client_model_names_resolve_to_the_catalog_slug() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-one",
            "model": "DeepSeek-V4.1-Flash[1m]",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(persisted_model(root, "session-one").as_deref(), Some(SLUG));

    let route = format!(
        "{}[1m]",
        codex_mixin::clients::claude_desktop::route_id(SLUG)
    );
    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-two",
            "model": route,
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(persisted_model(root, "session-two").as_deref(), Some(SLUG));
}

#[test]
fn session_start_seeds_the_model_for_later_events() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    run_hook(
        root,
        "session-start",
        &json!({
            "session_id": "session-three",
            "model": "DeepSeek-V4.1-Flash[1m]",
            "cwd": "/tmp",
        }),
    );
    assert!(root.join("ducx-report-models/session-three.json").is_file());

    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-three",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(
        persisted_model(root, "session-three").as_deref(),
        Some(SLUG)
    );
}

#[test]
fn usage_ledger_resolves_events_without_a_model() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    let connection = rusqlite::Connection::open(root.join("usage.sqlite3")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE token_usage_turns (
                at_ms INTEGER NOT NULL,
                session TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                catalog_slug TEXT NOT NULL
            );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO token_usage_turns (at_ms, session, provider_id, catalog_slug) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                1_i64,
                codex_mixin::usage_session_tag("session-four"),
                "baidu-oneapi",
                SLUG
            ],
        )
        .unwrap();
    drop(connection);

    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-four",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(persisted_model(root, "session-four").as_deref(), Some(SLUG));
}

#[test]
fn dsh_session_log_resolves_events_without_a_model() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    let session_dir = root
        .join("home/.dsh/sessions/--tmp-workspace--")
        .join("session-session-five");
    std::fs::create_dir_all(&session_dir).unwrap();
    let record = serde_json::to_vec(&json!({
        "type": "turn",
        "data": {"model": SLUG},
    }))
    .unwrap();
    let mut compressed = Vec::new();
    {
        let mut encoder = zstd::stream::write::Encoder::new(&mut compressed, 3).unwrap();
        encoder.write_all(&record).unwrap();
        encoder.write_all(b"\n").unwrap();
        encoder.finish().unwrap();
    }
    std::fs::write(session_dir.join("session.v4.jsonl.zstd"), compressed).unwrap();

    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-five",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(persisted_model(root, "session-five").as_deref(), Some(SLUG));
}

#[test]
fn model_switch_updates_the_cache_without_reporting() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    run_hook(
        root,
        "model-switch",
        &json!({
            "session_id": "session-six",
            "from_model": "Claude Haiku 5.5-baidu-oneapi[1m]",
            "to_model": SLUG,
            "cwd": "/tmp",
        }),
    );
    assert!(root.join("ducx-report-models/session-six.json").is_file());
    assert!(persisted_model(root, "session-six").is_none());

    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-six",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(persisted_model(root, "session-six").as_deref(), Some(SLUG));
}

#[test]
fn dsh_connect_mounts_and_removes_the_reporting_bridge() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    let dsh_home = root.join("home/.dsh");
    let patch_path = dsh_home.join("profiles/desktop/cordis.patch.yml");
    std::fs::create_dir_all(patch_path.parent().unwrap()).unwrap();
    std::fs::write(&patch_path, "- id: user-entry\n  config: {}\n").unwrap();
    // The bridge and its directly imported peers are pre-installed so the test
    // never shells out to a real DSH CLI/pnpm on machines that have DSH.
    for package in ["dsh-hooks-claude-code", "dsh-hook-protocol", "dsh-llm"] {
        let package_dir = dsh_home
            .join("profiles/node_modules/@deepseek-ai")
            .join(package);
        std::fs::create_dir_all(&package_dir).unwrap();
        std::fs::write(package_dir.join("package.json"), "{}").unwrap();
    }

    let output = common::cli(root)
        .args(["--no-tui", "--json-errors", "connect", "dsh"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "connect dsh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let patch = std::fs::read_to_string(&patch_path).unwrap();
    let parsed: serde_yaml::Value = serde_yaml::from_str(&patch).unwrap();
    let row = managed_row(&parsed).expect("reporting bridge row");
    assert_eq!(
        row.get("name").and_then(serde_yaml::Value::as_str),
        Some("@deepseek-ai/dsh-hooks-claude-code")
    );
    assert_eq!(
        row.get("config")
            .and_then(|config| config.get("configPath"))
            .and_then(serde_yaml::Value::as_str),
        dsh_home.join("hooks.json").to_str()
    );
    assert!(patch.contains("user-entry"), "patch: {patch}");

    // Re-running connect must not duplicate the managed row.
    let output = common::cli(root)
        .args(["--no-tui", "--json-errors", "connect", "dsh"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "second connect dsh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let patch = std::fs::read_to_string(&patch_path).unwrap();
    assert_eq!(patch.matches("codex-mixin-report-hooks").count(), 1);

    let output = common::cli(root)
        .args(["--no-tui", "--json-errors", "connect", "remove", "dsh"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "connect remove dsh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let patch = std::fs::read_to_string(&patch_path).unwrap();
    assert!(
        !patch.contains("codex-mixin-report-hooks"),
        "patch: {patch}"
    );
    assert!(patch.contains("user-entry"), "patch: {patch}");
}

#[cfg(unix)]
#[test]
fn read_only_state_dir_falls_back_to_the_temp_area() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);
    let temp_area = root.join("tmp");
    std::fs::create_dir_all(&temp_area).unwrap();

    let original = std::fs::metadata(root).unwrap().permissions();
    let mut read_only = original.clone();
    read_only.set_mode(0o500);
    std::fs::set_permissions(root, read_only).unwrap();

    run_hook_with(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-seven",
            "model": "DeepSeek-V4.1-Flash[1m]",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
        |command| {
            command.env("TMPDIR", &temp_area);
        },
    );

    std::fs::set_permissions(root, original).unwrap();

    let log = std::fs::read_to_string(temp_area.join("codex-mixin-report/report-hook.log"))
        .expect("fallback report log");
    assert!(log.contains("DUCX report event persisted"), "log: {log}");
}

fn managed_row(patch: &serde_yaml::Value) -> Option<&serde_yaml::Value> {
    patch.as_sequence()?.iter().find_map(|entry| {
        entry.get("insert")?.as_sequence()?.iter().find(|row| {
            row.get("id").and_then(serde_yaml::Value::as_str) == Some("codex-mixin-report-hooks")
        })
    })
}
