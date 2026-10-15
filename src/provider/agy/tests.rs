use super::*;
use serde_json::json;

#[test]
fn models_follow_the_report() {
    let models =
        parse_models(&json!({"models":[{"id":"new-upstream-id","label":"New model"}]})).unwrap();
    assert_eq!(models[0].id, "new-upstream-id");
    assert_eq!(models[0].supports_web_search, None);
    assert_eq!(models[0].supports_image, None);
    assert_eq!(models[0].context_window, None);
    assert!(parse_models(&json!({"models":[]})).unwrap().is_empty());
    let models = parse_models(
        &json!({"models":{"api-model":{"displayName":"API Model","inputTokenLimit":32768}}}),
    )
    .unwrap();
    assert_eq!(models[0].id, "api-model");
    assert_eq!(models[0].context_window, Some(32768));
}

#[test]
fn malformed_models_fail_closed() {
    for value in [
        json!({}),
        json!({"models":null}),
        json!({"models":[{"label":"no id"}]}),
        json!({"models":[{"id":"same"},{"id":"same"}]}),
        json!({"models":[{"id":"","label":"empty"}]}),
    ] {
        assert!(parse_models(&value).is_err());
    }
}

#[test]
fn quota_groups_are_dynamic() {
    let windows = parse_quota(&json!({"groups":[
        {"name":"Future family","buckets":[
            {"name":"Daily window","remaining_fraction":0.25,"reset_time":"2026-10-16T00:00:00Z"},
            {"name":"Inactive","enabled":false}
        ]},
        {"name":"Another family","buckets":[{"name":"Weekly","remaining_fraction":1.0}]}
    ]}))
    .unwrap();
    assert_eq!(windows.len(), 2);
    let daily = windows
        .iter()
        .find(|window| window.id == "Future family/Daily window")
        .unwrap();
    assert_eq!(daily.remaining_percent, 25.0);
    assert_eq!(daily.reset_at.as_deref(), Some("2026-10-16T00:00:00Z"));
}

#[test]
fn unavailable_quota_is_not_zero() {
    for fraction in [json!(null), json!(-0.1), json!(1.1)] {
        assert!(
            parse_quota(&json!({"groups":[{"name":"Family","buckets":[
                {"name":"Limit","remaining_fraction":fraction}
            ]}]}))
            .is_err()
        );
    }
    assert!(parse_quota(&json!({"groups":[]})).is_err());
    assert!(parse_quota(&json!({})).is_err());
}

#[test]
fn native_reports_are_account_bound() {
    let mut provider = super::super::agy_provider("test", "");
    assert!(require_native_account(&provider).is_ok());
    provider.base_url = "https://example.com".to_owned();
    assert!(require_native_account(&provider).is_err());
    provider.base_url = "https://cloudcode-pa.googleapis.com".to_owned();
    provider.auth.api_key = "another-account-key".to_owned();
    assert!(require_native_account(&provider).is_err());
}
