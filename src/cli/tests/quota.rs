use codex_mixin::provider::{ProviderPreset, ProviderRegistry};

use crate::cli::status::*;
use codex_mixin::provider::{QuotaUsageSummary, quota_usage};

#[test]
fn summarizes_generic_quota_shapes() {
    assert_eq!(
        summarize_quota_json(&serde_json::json!({"usage":{"used":"12.5","budget":100}})),
        "quota used: 12.5 / 100"
    );
    assert_eq!(
        summarize_quota_json(&serde_json::json!({"data":{"used":42}})),
        "quota used: 42"
    );
    assert_eq!(
        summarize_quota_json(
            &serde_json::json!({"data":{"used_quota":10,"month_quota_limit":50,"remaining_quota":40}})
        ),
        "quota used: 10 / 50, remaining: 40"
    );
}

#[test]
fn preserves_quota_limit_and_remaining_for_visualization() {
    assert_eq!(
        quota_usage(
            codex_mixin::provider::ProviderQuotaParser::BaiduOneApi,
            &serde_json::json!({
                "data": {
                    "used_quota": 10,
                    "month_quota_limit": 50,
                    "remaining_quota": 40
                }
            })
        )
        .unwrap(),
        QuotaUsageSummary {
            used: Some(10.0),
            limit: Some(50.0),
            remaining: Some(40.0),
            currency: None,
        }
    );
    assert_eq!(
        quota_usage(
            codex_mixin::provider::ProviderQuotaParser::OpenRouter,
            &serde_json::json!({"data":{"total_usage":12.5,"total_credits":100}})
        )
        .unwrap(),
        QuotaUsageSummary {
            used: Some(12.5),
            limit: Some(100.0),
            remaining: Some(87.5),
            currency: None,
        }
    );
    assert_eq!(
        quota_usage(
            codex_mixin::provider::ProviderQuotaParser::Generic,
            &serde_json::json!({
                "data": {
                    "total_used": "25.5",
                    "total_granted": 100,
                    "total_available": 74.5
                }
            })
        )
        .unwrap(),
        QuotaUsageSummary {
            used: Some(25.5),
            limit: Some(100.0),
            remaining: Some(74.5),
            currency: None,
        }
    );
    assert_eq!(
        quota_usage(
            codex_mixin::provider::ProviderQuotaParser::DeepSeek,
            &serde_json::json!({
                "is_available": true,
                "balance_infos": [{
                    "currency": "CNY",
                    "total_balance": "110.00",
                    "granted_balance": "10.00",
                    "topped_up_balance": "100.00"
                }]
            })
        )
        .unwrap(),
        QuotaUsageSummary {
            used: None,
            limit: None,
            remaining: Some(110.0),
            currency: Some("CNY".to_owned()),
        }
    );
}

#[test]
fn provider_presets_resolve_quota_urls() {
    let mut baidu = ProviderPreset::BaiduOneApi.create("baidu", "key");
    baidu.base_url = "https://oneapi.example".to_owned();
    baidu.quota_url = Some("https://oneapi.example/openapi/v3/user/quota".to_owned());
    baidu.quota_username = Some("quota-user".to_owned());
    let registry = ProviderRegistry::new(vec![baidu]).unwrap();
    assert_eq!(
        registry
            .provider("baidu")
            .unwrap()
            .quota_url()
            .unwrap()
            .as_str(),
        "https://oneapi.example/openapi/v3/user/quota?username=quota-user"
    );

    let openrouter = ProviderPreset::OpenRouter.create("openrouter", "key");
    let registry = ProviderRegistry::new(vec![openrouter]).unwrap();
    assert_eq!(
        registry
            .provider("openrouter")
            .unwrap()
            .quota_url()
            .unwrap()
            .as_str(),
        "https://openrouter.ai/api/v1/credits"
    );

    let deepseek = ProviderPreset::DeepSeek.create("deepseek", "key");
    let registry = ProviderRegistry::new(vec![deepseek]).unwrap();
    assert_eq!(
        registry
            .provider("deepseek")
            .unwrap()
            .quota_url()
            .unwrap()
            .as_str(),
        "https://api.deepseek.com/user/balance"
    );
}
