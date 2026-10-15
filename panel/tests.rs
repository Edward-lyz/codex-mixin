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
            json!({"action": "connect", "params": {"client": "codex_custom_only"}}),
            vec!["connect", "codex", "--custom-only"],
        ),
        (
            json!({"action": "disconnect", "params": {"client": "claude_desktop"}}),
            vec!["connect", "remove", "claude-desktop"],
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
