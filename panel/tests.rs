use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::json;
use tower::ServiceExt;

use super::actions::PanelAction;
use super::api::{PanelState, router, token_matches};

fn action(value: serde_json::Value) -> PanelAction {
    serde_json::from_value(value).unwrap()
}

fn test_router() -> axum::Router {
    router(std::sync::Arc::new(PanelState {
        token: "expected-token".to_owned(),
        start_page: "home",
        installed_cli_path: None,
        action_lock: tokio::sync::Mutex::new(()),
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
    ] {
        let value = json!({"action": "provider_update", "params": {"id": "p", "options": options}});
        assert!(action(value).invocation().is_err());
    }
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
