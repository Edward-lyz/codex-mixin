//! Parsing provider quota/balance responses.
//!
//! Each preset declares a ProviderQuotaParser; this module turns the raw
//! JSON that a quota endpoint returned into a uniform usage summary.

use super::ProviderQuotaParser;

#[derive(Clone, Debug, PartialEq)]
pub struct QuotaUsageSummary {
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub currency: Option<String>,
}

pub fn quota_usage(
    parser: ProviderQuotaParser,
    value: &serde_json::Value,
) -> anyhow::Result<QuotaUsageSummary> {
    if parser == ProviderQuotaParser::DeepSeek {
        return deepseek_quota_usage(value);
    }
    let (used_fields, limit_fields, remaining_fields): (&[&str], &[&str], &[&str]) = match parser {
        ProviderQuotaParser::BaiduOneApi => (
            &["used_quota", "used", "usage"],
            &[
                "month_quota_limit",
                "quota_limit",
                "limit",
                "total",
                "quota",
            ],
            &["remaining_quota", "remaining", "available"],
        ),
        ProviderQuotaParser::OpenRouter => (
            &["total_usage", "used", "usage"],
            &["total_credits", "limit", "total", "budget"],
            &["remaining", "remaining_quota", "available"],
        ),
        ProviderQuotaParser::Generic => (
            &[
                "used",
                "used_quota",
                "usage",
                "total_usage",
                "total_used",
                "spent",
                "cost",
                "consumed",
                "actual_cost",
            ],
            &[
                "limit",
                "total",
                "total_credits",
                "total_granted",
                "quota",
                "quota_limit",
                "month_quota_limit",
                "budget",
            ],
            &[
                "remaining",
                "remaining_quota",
                "available",
                "total_available",
                "balance",
            ],
        ),
        ProviderQuotaParser::DeepSeek => unreachable!("DeepSeek quota handled above"),
        ProviderQuotaParser::OpenCodeGo => {
            unreachable!("OpenCode Go quota is fetched from the dashboard HTML")
        }
    };
    let used = first_quota_value(value, used_fields)
        .ok_or_else(|| anyhow::anyhow!("quota response does not contain a valid used amount"))?;
    let reported_remaining = first_quota_value(value, remaining_fields);
    let limit = first_quota_value(value, limit_fields)
        .or_else(|| reported_remaining.map(|remaining| used + remaining));
    let remaining = reported_remaining.or_else(|| limit.map(|limit| (limit - used).max(0.0)));
    Ok(QuotaUsageSummary {
        used: Some(used),
        limit,
        remaining,
        currency: None,
    })
}

fn deepseek_quota_usage(value: &serde_json::Value) -> anyhow::Result<QuotaUsageSummary> {
    let balances = value
        .get("balance_infos")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("DeepSeek quota response has no balance_infos array"))?;
    let parsed = balances
        .iter()
        .filter_map(|entry| {
            let amount = entry.get("total_balance").and_then(json_f64)?;
            (amount.is_finite() && amount >= 0.0).then(|| {
                let currency = entry
                    .get("currency")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|currency| {
                        currency.len() == 3
                            && currency.bytes().all(|byte| byte.is_ascii_alphabetic())
                    })
                    .map(str::to_ascii_uppercase);
                (amount, currency)
            })
        })
        .collect::<Vec<_>>();
    let balance = parsed
        .iter()
        .find(|(amount, _)| *amount > 0.0)
        .or_else(|| parsed.first())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("DeepSeek quota response has no valid total_balance"))?;
    Ok(QuotaUsageSummary {
        used: None,
        limit: None,
        remaining: Some(balance.0),
        currency: balance.1,
    })
}

fn first_quota_value(value: &serde_json::Value, fields: &[&str]) -> Option<f64> {
    [
        "",
        "/data",
        "/quota",
        "/data/quota",
        "/usage",
        "/data/usage",
        "/usage/total",
        "/data/usage/total",
    ]
    .iter()
    .find_map(|base| {
        fields.iter().find_map(|field| {
            let pointer = if base.is_empty() {
                format!("/{field}")
            } else {
                format!("{base}/{field}")
            };
            value.pointer(&pointer).and_then(json_f64)
        })
    })
    .filter(|value| value.is_finite() && *value >= 0.0)
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}
