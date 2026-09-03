use codex_mixin::provider::catalog_model_slug;

use super::queue::{enqueue_at, load_pending_at, mark_delivered_at};
use super::transport::{provider_owns_model, redact_sensitive_response_body, truncate_for_log};
use super::*;

#[test]
fn parses_git_remotes_into_repo_paths() {
    assert_eq!(
        parse_remote_repo("git@github.com:Edward-lyz/codex-mixin.git"),
        "Edward-lyz/codex-mixin"
    );
    assert_eq!(
        parse_remote_repo("https://github.com/Edward-lyz/codex-mixin.git"),
        "Edward-lyz/codex-mixin"
    );
    assert_eq!(
        parse_remote_repo("ssh://git@icode.baidu.com/user/Work"),
        "user/Work"
    );
}

#[test]
fn reads_model_from_hook_body() {
    let body = br#"{"model":"gpt-5.6-luna","cwd":"/tmp"}"#;
    assert_eq!(hook_body_model(body), Some("gpt-5.6-luna".to_owned()));
    assert_eq!(hook_body_model(br#"{"cwd":"/tmp"}"#), None);
}

#[test]
fn filters_code_upload_to_apply_patch() {
    assert!(is_apply_patch_tool(br#"{"tool_name":"apply_patch"}"#));
    assert!(!is_apply_patch_tool(br#"{"tool_name":"Bash"}"#));
    assert!(!is_apply_patch_tool(
        br#"{"tool_name":"mcp__codex__apply_patch"}"#
    ));
}

#[test]
fn permits_session_scoped_uploads_only_after_a_successful_query() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        !successful_query_marker_exists_at(directory.path(), "provider-1", "session-1").unwrap()
    );

    record_successful_query_marker_at(directory.path(), "provider-1", "session-1").unwrap();

    assert!(
        successful_query_marker_exists_at(directory.path(), "provider-1", "session-1").unwrap()
    );
    assert!(
        !successful_query_marker_exists_at(directory.path(), "provider-2", "session-1").unwrap()
    );
}

#[test]
fn rejects_unsafe_session_ids_for_local_state() {
    let directory = tempfile::tempdir().unwrap();
    assert!(successful_query_marker_path(directory.path(), "provider", "../session").is_err());
    assert!(successful_query_marker_path(directory.path(), "../provider", "session").is_err());
}

#[test]
fn builds_query_payload_from_hook_body() {
    let body = br#"{"session_id":"sess","model":"mixin/m","prompt":"hello","cwd":"/tmp"}"#;
    let payload = query_payload(body, "user");
    assert_eq!(payload["session_id"], "sess");
    assert_eq!(payload["username"], "user");
    assert_eq!(payload["query"], "hello");
    assert_eq!(payload["model"], "mixin/m");
}

#[test]
fn scopes_reporting_to_provider_models() {
    let mut provider = codex_mixin::provider::baidu_oneapi_provider("baidu-oneapi", "key");
    provider.selected_models = vec!["GLM-5.2".to_owned()];
    provider.cached_models = vec![codex_mixin::provider::ProviderModel {
        id: "Opus 5".to_owned(),
        ..Default::default()
    }];
    assert!(!provider_owns_model(&provider, "GLM-5.2"));
    assert!(!provider_owns_model(&provider, "Opus 5"));
    assert!(provider_owns_model(
        &provider,
        &catalog_model_slug("GLM-5.2", "baidu-oneapi")
    ));
    assert!(!provider_owns_model(
        &provider,
        &catalog_model_slug("Opus 5", "baidu-oneapi")
    ));
    assert!(!provider_owns_model(&provider, "gpt-4o"));
}

#[test]
fn truncates_response_bodies_for_logs() {
    assert_eq!(truncate_for_log("short", 10), "short");
    let long = "x".repeat(3_000);
    let truncated = truncate_for_log(&long, 16);
    assert!(truncated.starts_with("xxxxxxxxxxxxxxxx..."));
    assert!(truncated.contains("3000 chars total"));
}

#[test]
fn redacts_signed_file_urls_from_response_logs() {
    let response =
        "https://example.test/file?authorization=secret&x-amz-signature=also-secret&keep=1";
    assert_eq!(
        redact_sensitive_response_body(response).unwrap(),
        "https://example.test/file?authorization=<redacted>&x-amz-signature=<redacted>&keep=1"
    );
}

#[test]
fn report_queue_deduplicates_and_remembers_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let body = br#"{"session_id":"session-1","model":"model-1","prompt":"hello"}"#;

    let first = enqueue_at(
        directory.path(),
        "user-prompt-submit",
        "baidu-oneapi",
        "turn-1",
        body,
    )
    .unwrap();
    let second = enqueue_at(
        directory.path(),
        "user-prompt-submit",
        "baidu-oneapi",
        "turn-1",
        body,
    )
    .unwrap();
    enqueue_at(
        directory.path(),
        "user-prompt-submit",
        "baidu-oneapi",
        "turn-2",
        body,
    )
    .unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(load_pending_at(directory.path()).unwrap().len(), 2);

    let pending = load_pending_at(directory.path()).unwrap();
    let first_record = pending.iter().find(|record| record.id == first.id).unwrap();
    mark_delivered_at(directory.path(), first_record).unwrap();
    assert_eq!(load_pending_at(directory.path()).unwrap().len(), 1);
    assert!(
        enqueue_at(
            directory.path(),
            "user-prompt-submit",
            "baidu-oneapi",
            "turn-1",
            body,
        )
        .unwrap()
        .already_delivered
    );
}

#[test]
fn report_queue_keeps_provider_identities_separate() {
    let directory = tempfile::tempdir().unwrap();
    let body = br#"{"session_id":"session-1","model":"shared-model"}"#;

    enqueue_at(directory.path(), "stop", "baidu-oneapi", "stop-1", body).unwrap();
    enqueue_at(directory.path(), "stop", "baidu-oneapi-2", "stop-1", body).unwrap();

    let pending = load_pending_at(directory.path()).unwrap();
    assert_eq!(pending.len(), 2);
    assert_ne!(pending[0].id, pending[1].id);
}

#[test]
fn local_session_replay_uses_the_last_user_message_before_each_turn() {
    let directory = tempfile::tempdir().unwrap();
    let session_path = directory.path().join("session.jsonl");
    std::fs::write(
            &session_path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session-1\",\"cwd\":\"/repo\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"injected context\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"actual prompt\"}]}}\n",
                "{\"type\":\"turn_context\",\"payload\":{\"model\":\"model-1-baidu-oneapi\",\"cwd\":\"/repo\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"official prompt\"}]}}\n",
                "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-official\",\"cwd\":\"/repo\"}}\n"
            ),
        )
        .unwrap();
    let mut provider = codex_mixin::provider::baidu_oneapi_provider("baidu-oneapi", "key");
    provider.enabled = true;
    provider.request_policy.baidu_code_report = true;
    provider.selected_models = vec!["model-1".to_owned()];

    assert_eq!(
        enqueue_session_file(&session_path, &[provider], directory.path()).unwrap(),
        2
    );
    let pending = load_pending_at(directory.path()).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].event, "user-prompt-submit");
    assert_eq!(pending[0].hook_body["prompt"], "actual prompt");
    assert_eq!(pending[1].event, "stop");
    assert_eq!(
        pending[1].hook_body["transcript_path"],
        session_path.to_string_lossy().as_ref()
    );
}

#[test]
fn partial_replay_json_preserves_successes_and_failures() {
    let report = ReplayReport {
        queued_from_local_sessions: 3,
        delivered: vec![ReplayEvent {
            provider_id: "baidu-oneapi".to_owned(),
            session_id: "session-ok".to_owned(),
            event: "post-tool-use".to_owned(),
        }],
        retained: vec![ReplayFailure {
            provider_id: "baidu-oneapi".to_owned(),
            session_id: "session-failed".to_owned(),
            event: "post-tool-use".to_owned(),
            error: "DUCX report endpoint upload/code/accept returned 500 Internal Server Error"
                .to_owned(),
        }],
    };

    let value = serde_json::to_value(report).unwrap();
    assert_eq!(value["queued_from_local_sessions"], 3);
    assert_eq!(value["delivered"][0]["session_id"], "session-ok");
    assert_eq!(value["retained"][0]["session_id"], "session-failed");
    assert!(
        value["retained"][0]["error"]
            .as_str()
            .unwrap()
            .contains("upload/code/accept returned 500")
    );
}
