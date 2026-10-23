use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::json;
use tower::ServiceExt;

use super::actions::PanelAction;
use super::api::{PanelState, router, token_matches};

#[tokio::test]
async fn default_panel_port_stays_in_range() {
    let listener = super::bind_listener(None).await.unwrap();
    let address = listener.local_addr().unwrap();
    assert!(address.ip().is_unspecified());
    assert!((8000..=9000).contains(&address.port()));
}

#[tokio::test]
async fn explicit_panel_bind_fails_when_occupied() {
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = occupied.local_addr().unwrap();
    let error = super::bind_listener(Some(address)).await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::AddrInUse
    );
}

fn action(value: serde_json::Value) -> PanelAction {
    serde_json::from_value(value).unwrap()
}

fn test_router() -> axum::Router {
    router(std::sync::Arc::new(PanelState {
        token: "expected-token".to_owned(),
        start_page: "home",
        installed_cli_path: None,
        action_lock: tokio::sync::Mutex::new(()),
        cancel_action: tokio::sync::watch::channel(0).0,
    }))
}

#[test]
fn token_check_requires_the_exact_bearer_token() {
    assert!(token_matches("secret", Some("Bearer secret")));
    assert!(!token_matches("secret", Some("Bearer secret2")));
    assert!(!token_matches("secret", Some("secret")));
    assert!(!token_matches("secret", Some("Bearer ")));
    assert!(!token_matches("secret", None));
}

#[tokio::test]
async fn api_rejects_requests_without_the_token_before_running_the_cli() {
    for authorization in [None, Some("Bearer wrong-token")] {
        let mut request = Request::get("/api/snapshot");
        if let Some(value) = authorization {
            request = request.header(header::AUTHORIZATION, value);
        }
        let response = test_router()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn usage_details_rejects_unlisted_ranges_before_running_the_cli() {
    for days in ["0", "365", "--help", "7%20--limit"] {
        let response = test_router()
            .oneshot(
                Request::get(format!("/api/usage?days={days}"))
                    .header(header::AUTHORIZATION, "Bearer expected-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{days}");
    }
}

#[tokio::test]
async fn static_assets_are_public_and_carry_a_strict_policy() {
    let response = test_router()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(policy.contains("script-src 'self'"));
    assert!(policy.contains("frame-ancestors 'none'"));
}

#[test]
fn provider_ids_that_look_like_flags_are_rejected() {
    for value in [
        json!({"action": "provider_enable", "params": {"id": "--help"}}),
        json!({"action": "provider_remove", "params": {"id": ""}}),
        json!({"action": "provider_reorder", "params": {"ids": ["a", "-b"]}}),
        json!({"action": "provider_test", "params": {"id": "a\nb"}}),
    ] {
        assert!(action(value).invocation().is_err());
    }
}

#[test]
fn secrets_travel_through_the_environment_only() {
    let invocation = action(json!({
        "action": "provider_add",
        "params": {
            "preset": "deepseek",
            "options": {"key": "sk-live-value", "display_name": "Deep", "model": ["m1", "m2"]}
        }
    }))
    .invocation()
    .unwrap();
    assert!(
        invocation
            .args
            .iter()
            .all(|arg| !arg.contains("sk-live-value"))
    );
    assert_eq!(
        invocation.args,
        [
            "providers",
            "add",
            "--preset=deepseek",
            "--display-name=Deep",
            "--key=@env:CODEX_MIXIN_PANEL_SECRET_0",
            "--model=m1",
            "--model=m2",
        ]
    );
    assert_eq!(invocation.secrets.len(), 1);
    assert_eq!(invocation.secrets[0].value, "sk-live-value");
}

#[test]
fn provider_options_are_allowlisted_and_cannot_reference_panel_env() {
    for options in [
        json!({"ducx_executable": "/bin/sh"}),
        json!({"display_name": "@env:HOME"}),
        json!({"clear_key": "yes"}),
        json!({"auxiliary_model_upstream": true}),
    ] {
        let value = json!({"action": "provider_update", "params": {"id": "p", "options": options}});
        assert!(action(value).invocation().is_err());
    }
}

#[test]
fn provider_model_overrides_use_independent_cli_options() {
    let invocation = action(json!({
        "action": "provider_update", "params": {"id": "p", "options": {
            "auto_review_model": "review", "codex_thread_title_model": "title"
        }}
    }))
    .invocation()
    .unwrap();
    assert_eq!(
        invocation.args,
        [
            "providers",
            "update",
            "p",
            "--auto-review-model=review",
            "--codex-thread-title-model=title"
        ]
    );
    let invocation = action(json!({
        "action": "provider_update", "params": {"id": "p", "options": {
            "clear_auto_review_model": true, "clear_codex_thread_title_model": true
        }}
    }))
    .invocation()
    .unwrap();
    assert_eq!(
        invocation.args,
        [
            "providers",
            "update",
            "p",
            "--clear-auto-review-model",
            "--clear-codex-thread-title-model"
        ]
    );
}

#[test]
fn fixed_actions_map_to_their_cli_commands() {
    let cases = [
        (
            json!({"action": "service_restart"}),
            vec!["service", "restart"],
        ),
        (
            json!({"action": "agent_connect", "params": {"id": "codex", "mode": "custom_only"}}),
            vec!["agent", "connect", "codex", "--mode=custom-only"],
        ),
        (
            json!({"action": "agent_disconnect", "params": {"id": "claude-desktop"}}),
            vec!["agent", "disconnect", "claude-desktop"],
        ),
        (
            json!({"action": "provider_select", "params": {
                "id": "p", "models": ["m"], "model_contexts": {"m": 200000}
            }}),
            vec![
                "providers",
                "select",
                "p",
                "--model=m",
                "--model-context=m=200000",
            ],
        ),
    ];
    for (value, expected) in cases {
        assert_eq!(action(value).invocation().unwrap().args, expected);
    }
}

#[test]
fn context_settings_map_to_the_context_cli() {
    let invocation = action(json!({
        "action": "context_configure",
        "params": {
            "enabled": true,
            "growth_threshold_tokens": 60000,
            "protected_messages": 3,
            "max_internal_rounds": 2,
            "context_window_tokens": "200000"
        }
    }))
    .invocation()
    .unwrap();
    assert_eq!(
        invocation.args,
        [
            "context",
            "config",
            "set",
            "--enabled=true",
            "--growth-threshold-tokens=60000",
            "--protected-messages=3",
            "--max-internal-rounds=2",
            "--context-window-tokens=200000",
            "--json",
        ]
    );
}

#[test]
fn context_settings_reject_invalid_values() {
    for value in [
        json!({"action": "context_configure", "params": {"enabled": true, "growth_threshold_tokens": 0}}),
        json!({"action": "context_configure", "params": {"enabled": true, "protected_messages": 0}}),
        json!({"action": "context_configure", "params": {"enabled": true, "max_internal_rounds": 11}}),
        json!({"action": "context_configure", "params": {"enabled": true, "context_window_tokens": "1000\n2000"}}),
    ] {
        assert!(action(value).invocation().is_err());
    }
}

#[tokio::test]
async fn context_stats_requires_the_panel_token() {
    let response = test_router()
        .oneshot(Request::get("/api/context").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn agent_icons_match_the_shared_manifest() {
    let manifest: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(include_str!("../macos/assets/agents/manifest.json")).unwrap();
    let icons = super::agent_icons::AGENT_ICONS
        .iter()
        .map(|(id, _, _)| *id)
        .collect::<Vec<_>>();
    let mut expected = manifest.keys().map(String::as_str).collect::<Vec<_>>();
    expected.sort_unstable();
    assert_eq!(icons, expected);
    let mut templates = manifest
        .iter()
        .filter(|(_, entry)| entry["template"] == true)
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>();
    templates.sort_unstable();
    assert_eq!(super::agent_icons::TEMPLATE_AGENT_ICONS, templates);
}

#[tokio::test]
async fn agent_icons_are_served_and_unknown_ids_are_not_found() {
    let response = test_router()
        .oneshot(
            Request::get("/agent-icons/codex")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
    let missing = test_router()
        .oneshot(
            Request::get("/agent-icons/..%2Fsecret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[test]
fn manual_report_runs_warmup_restart_and_replay_in_order() {
    let steps = action(json!({"action": "report_replay"}))
        .invocations()
        .unwrap()
        .into_iter()
        .map(|step| step.args)
        .collect::<Vec<_>>();
    assert_eq!(
        steps,
        [
            vec!["report-replay", "--prepare-warmup"],
            vec!["service", "restart"],
            vec!["report-replay", "--all-sessions", "--json"],
        ]
    );
}

#[tokio::test]
async fn empty_backup_import_is_rejected_before_running_the_cli() {
    let response = test_router()
        .oneshot(
            Request::post("/api/config/import")
                .header(header::AUTHORIZATION, "Bearer expected-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn provider_icons_are_served_by_preset_icon_name() {
    for (path, status) in [
        ("/provider-icons/baidu", StatusCode::OK),
        ("/provider-icons/unknown", StatusCode::NOT_FOUND),
    ] {
        let response = test_router()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{path}");
    }
}

#[test]
fn oauth_uses_the_add_preset_contract() {
    let invocation = action(json!({"action":"provider_add","params":{
        "preset":"openai","options":{"oauth_timeout_seconds":"120"}
    }}))
    .invocation()
    .unwrap();
    assert_eq!(&invocation.args[..2], ["providers", "add"]);
    assert!(invocation.args.iter().any(|arg| arg == "--preset=openai"));
    assert!(
        invocation
            .args
            .iter()
            .any(|arg| arg == "--oauth-timeout-seconds=120")
    );
    assert!(
        serde_json::from_value::<PanelAction>(
            json!({"action":"provider_login","params":{"id":"official"}})
        )
        .is_err()
    );
}

#[tokio::test]
async fn cancellation_requires_panel_authentication() {
    let denied = test_router()
        .oneshot(
            Request::post("/api/actions/cancel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let state = std::sync::Arc::new(PanelState {
        token: "expected-token".to_owned(),
        start_page: "home",
        installed_cli_path: None,
        action_lock: tokio::sync::Mutex::new(()),
        cancel_action: tokio::sync::watch::channel(0).0,
    });
    let cancellation = state.cancel_action.subscribe();
    let accepted = router(state.clone())
        .oneshot(
            Request::post("/api/actions/cancel")
                .header(header::AUTHORIZATION, "Bearer expected-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    assert!(cancellation.has_changed().unwrap());
}
