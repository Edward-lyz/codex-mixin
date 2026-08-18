use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use codex_mixin::provider::ProviderDefinition;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use regex::Regex;

pub(crate) const OPENCODE_GO_DASHBOARD_BASE: &str = "https://opencode.ai";
const OPENCODE_GO_SCRAPE_TIMEOUT: Duration = Duration::from_secs(10);
const OPENCODE_GO_UNITS_PER_USD: f64 = 100_000_000.0;
const OPENCODE_GO_PATH_ENCODE: &AsciiSet =
    &CONTROLS.add(b' ').add(b'/').add(b'?').add(b'#').add(b'%');

#[derive(Clone, Debug)]
pub(crate) struct OpenCodeGoWindowUsage {
    pub(super) quota_id: &'static str,
    pub(super) label: &'static str,
    pub(super) used_percent: f64,
    pub(super) reset_in_sec: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct OpenCodeGoBilling {
    pub(super) balance_usd: f64,
    pub(super) monthly_limit_usd: Option<f64>,
    pub(super) monthly_usage_usd: Option<f64>,
}

pub(crate) async fn fetch_opencode_go_quota_results(
    client: &reqwest::Client,
    provider: &ProviderDefinition,
    dashboard_base: &str,
) -> Vec<serde_json::Value> {
    let Some(workspace_id) = provider
        .quota_workspace_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return vec![opencode_go_error_result(
            provider,
            provider.display_name.clone(),
            "quota endpoint is not configured".to_owned(),
        )];
    };
    let Some(auth_cookie) = provider
        .quota_auth_cookie
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return vec![opencode_go_error_result(
            provider,
            provider.display_name.clone(),
            "quota endpoint is not configured".to_owned(),
        )];
    };
    let encoded_workspace_id =
        utf8_percent_encode(workspace_id, OPENCODE_GO_PATH_ENCODE).to_string();
    let usage_url = format!("{dashboard_base}/workspace/{encoded_workspace_id}/go");
    let billing_url = format!("{dashboard_base}/workspace/{encoded_workspace_id}/billing");
    let (usage_result, billing_result) = tokio::join!(
        fetch_opencode_go_usage(client, &usage_url, auth_cookie),
        fetch_opencode_go_billing(client, &billing_url, auth_cookie),
    );
    let mut results = Vec::new();
    match usage_result {
        Ok(windows) => {
            for window in windows {
                let used = window.used_percent.clamp(0.0, 100.0);
                results.push(serde_json::json!({
                    "provider_id": provider.id,
                    "provider_display_name": provider.display_name,
                    "display_name": format!("{} {}", provider.display_name, window.label),
                    "quota_id": window.quota_id,
                    "label": window.label,
                    "currency": null,
                    "value": used,
                    "used": used,
                    "limit": 100.0,
                    "remaining": (100.0 - used).max(0.0),
                    "error": null,
                    "stale_at": null,
                    "reset_at": unix_time_plus_seconds(window.reset_in_sec),
                }));
            }
        }
        Err(error) => results.push(opencode_go_error_result(
            provider,
            "OpenCode Go".to_owned(),
            error,
        )),
    }
    match billing_result {
        Ok(billing) => results.push(serde_json::json!({
            "provider_id": provider.id,
            "provider_display_name": provider.display_name,
            "display_name": format!("{} Balance", provider.display_name),
            "quota_id": "balance",
            "label": "Balance",
            "currency": "USD",
            "value": null,
            "used": null,
            "limit": billing.monthly_limit_usd,
            "remaining": billing.balance_usd,
            "monthly_usage": billing.monthly_usage_usd,
            "error": null,
            "stale_at": null,
        })),
        Err(error) => results.push(opencode_go_error_result(
            provider,
            "OpenCode Go Balance".to_owned(),
            error,
        )),
    }
    results
}

fn opencode_go_error_result(
    provider: &ProviderDefinition,
    display_name: String,
    error: String,
) -> serde_json::Value {
    serde_json::json!({
        "provider_id": provider.id,
        "provider_display_name": provider.display_name,
        "display_name": display_name,
        "quota_id": "quota",
        "label": "Quota",
        "currency": provider.quota_currency,
        "value": null,
        "error": error,
        "stale_at": null,
    })
}

async fn fetch_opencode_go_usage(
    client: &reqwest::Client,
    url: &str,
    auth_cookie: &str,
) -> Result<Vec<OpenCodeGoWindowUsage>, String> {
    let html = fetch_opencode_go_html(client, url, auth_cookie).await?;
    parse_opencode_go_usage_html(&html)
        .ok_or_else(|| "could not parse OpenCode Go dashboard usage".to_owned())
}

async fn fetch_opencode_go_billing(
    client: &reqwest::Client,
    url: &str,
    auth_cookie: &str,
) -> Result<OpenCodeGoBilling, String> {
    let html = fetch_opencode_go_html(client, url, auth_cookie).await?;
    parse_opencode_go_billing_html(&html)
        .ok_or_else(|| "could not parse OpenCode Go billing data".to_owned())
}

async fn fetch_opencode_go_html(
    client: &reqwest::Client,
    url: &str,
    auth_cookie: &str,
) -> Result<String, String> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "text/html")
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Gecko/20100101 Firefox/148.0",
        )
        .header(reqwest::header::COOKIE, format!("auth={auth_cookie}"))
        .timeout(OPENCODE_GO_SCRAPE_TIMEOUT)
        .send()
        .await
        .map_err(|error| redact_opencode_go_message(&error.to_string(), auth_cookie))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("OpenCode Go dashboard error {status}"));
    }
    response
        .text()
        .await
        .map_err(|error| redact_opencode_go_message(&error.to_string(), auth_cookie))
}

fn redact_opencode_go_message(message: &str, auth_cookie: &str) -> String {
    let mut sanitized = message
        .replace(auth_cookie, "<redacted>")
        .replace(['\n', '\r'], " ");
    sanitized = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    if sanitized.len() > 240 {
        sanitized.truncate(240);
    }
    sanitized
}

pub(crate) fn parse_opencode_go_usage_html(html: &str) -> Option<Vec<OpenCodeGoWindowUsage>> {
    let ssr = [
        ("five_hour", "5h", "rollingUsage"),
        ("weekly", "Weekly", "weeklyUsage"),
        ("monthly", "Monthly", "monthlyUsage"),
    ]
    .into_iter()
    .filter_map(|(quota_id, label, field)| {
        parse_opencode_go_ssr_window(html, field).map(|(used_percent, reset_in_sec)| {
            OpenCodeGoWindowUsage {
                quota_id,
                label,
                used_percent,
                reset_in_sec,
            }
        })
    })
    .collect::<Vec<_>>();
    if !ssr.is_empty() {
        return Some(ssr);
    }
    let data_slot = parse_opencode_go_data_slot_windows(html);
    if data_slot.is_empty() {
        return None;
    }
    let mut windows = Vec::new();
    for (key, quota_id, label) in [
        ("rolling", "five_hour", "5h"),
        ("weekly", "weekly", "Weekly"),
        ("monthly", "monthly", "Monthly"),
    ] {
        if let Some(window) = data_slot.get(key) {
            windows.push(OpenCodeGoWindowUsage {
                quota_id,
                label,
                used_percent: window.0,
                reset_in_sec: window.1,
            });
        }
    }
    (!windows.is_empty()).then_some(windows)
}

fn parse_opencode_go_ssr_window(html: &str, field: &str) -> Option<(f64, f64)> {
    let number = r"(-?\d+(?:\.\d+)?)";
    let usage_first = Regex::new(&format!(
        r"{field}:\$R\[\d+\]=\{{[^}}]*usagePercent:{number}[^}}]*resetInSec:{number}[^}}]*\}}"
    ))
    .ok()?;
    if let Some(captures) = usage_first.captures(html) {
        let usage = captures.get(1)?.as_str().parse::<f64>().ok()?;
        let reset = captures.get(2)?.as_str().parse::<f64>().ok()?;
        if usage.is_finite() && reset.is_finite() {
            return Some((usage, reset));
        }
    }
    let reset_first = Regex::new(&format!(
        r"{field}:\$R\[\d+\]=\{{[^}}]*resetInSec:{number}[^}}]*usagePercent:{number}[^}}]*\}}"
    ))
    .ok()?;
    let captures = reset_first.captures(html)?;
    let reset = captures.get(1)?.as_str().parse::<f64>().ok()?;
    let usage = captures.get(2)?.as_str().parse::<f64>().ok()?;
    (usage.is_finite() && reset.is_finite()).then_some((usage, reset))
}

fn parse_opencode_go_data_slot_windows(html: &str) -> HashMap<String, (f64, f64)> {
    let mut windows = HashMap::new();
    let usage_number = Regex::new(r"\d+(?:\.\d+)?").ok();
    for item in html.split("data-slot=\"usage-item\"") {
        let label = item
            .split("data-slot=\"usage-label\">")
            .nth(1)
            .and_then(|value| value.split('<').next())
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let usage = match item
            .split("data-slot=\"usage-value\">")
            .nth(1)
            .and_then(|value| usage_number.as_ref()?.find(value))
            .and_then(|matched| matched.as_str().parse::<f64>().ok())
            .filter(|value| value.is_finite())
        {
            Some(value) => value,
            None => continue,
        };
        let reset = match item
            .split("data-slot=\"reset-now\">")
            .nth(1)
            .map(|_| 0.0)
            .or_else(|| {
                item.split("data-slot=\"reset-time\">")
                    .nth(1)
                    .and_then(|value| value.split("</span>").next())
                    .and_then(parse_opencode_go_reset_time)
            }) {
            Some(value) => value,
            None => continue,
        };
        let key = if label.contains("rolling") {
            "rolling"
        } else if label.contains("weekly") {
            "weekly"
        } else if label.contains("monthly") {
            "monthly"
        } else {
            continue;
        };
        windows.insert(key.to_owned(), (usage, reset));
    }
    windows
}

fn parse_opencode_go_reset_time(value: &str) -> Option<f64> {
    let normalized = value
        .to_ascii_lowercase()
        .replace("resets in", "")
        .replace("reset in", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.contains("now") {
        return Some(0.0);
    }
    let mut total = 0.0;
    let mut found = false;
    for (multiplier, suffix) in [
        (86400.0, r"days?"),
        (3600.0, r"hours?"),
        (60.0, r"minutes?"),
        (1.0, r"seconds?"),
    ] {
        if let Some(captures) = Regex::new(&format!(r"(\d+(?:\.\d+)?)\s*{suffix}"))
            .ok()?
            .captures(&normalized)
        {
            let value = captures.get(1)?.as_str().parse::<f64>().ok()?;
            total += value * multiplier;
            found = true;
        }
    }
    found.then_some(total)
}

pub(crate) fn parse_opencode_go_billing_html(html: &str) -> Option<OpenCodeGoBilling> {
    let mut fields = HashMap::new();
    let field_re =
        Regex::new(r"\b(balance|monthlyLimit|monthlyUsage)\s*:\s*(\d+(?:\.\d+)?)\b").ok()?;
    for captures in field_re.captures_iter(html) {
        fields.insert(
            captures.get(1)?.as_str().to_owned(),
            captures.get(2)?.as_str().parse::<f64>().ok()?,
        );
    }
    if let Some(balance_units) = fields.get("balance").copied() {
        let balance_usd = balance_units / OPENCODE_GO_UNITS_PER_USD;
        let monthly_limit_usd = fields.get("monthlyLimit").copied();
        let monthly_usage_usd = fields
            .get("monthlyUsage")
            .copied()
            .map(|units| units / OPENCODE_GO_UNITS_PER_USD);
        if balance_usd.is_finite() && balance_usd >= 0.0 {
            return Some(OpenCodeGoBilling {
                balance_usd,
                monthly_limit_usd,
                monthly_usage_usd,
            });
        }
    }
    parse_opencode_go_data_slot_billing(html)
}

fn parse_opencode_go_data_slot_billing(html: &str) -> Option<OpenCodeGoBilling> {
    let mut balance_usd = None;
    let mut monthly_limit_usd = None;
    let mut monthly_usage_usd = None;
    let dollar_amount = Regex::new(r"\$?\s*(\d+(?:,\d{3})*(?:\.\d+)?)").ok()?;
    for item in html.split("data-slot=\"billing-item\"") {
        let label = item
            .split("data-slot=\"billing-label\">")
            .nth(1)
            .and_then(|value| value.split('<').next())
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let Some(value) = item
            .split("data-slot=\"billing-value\">")
            .nth(1)
            .and_then(|value| dollar_amount.captures(value))
            .and_then(|captures| captures.get(1))
            .and_then(|matched| matched.as_str().replace(',', "").parse::<f64>().ok())
        else {
            continue;
        };
        if !value.is_finite() || value < 0.0 {
            continue;
        }
        if label.contains("balance") {
            balance_usd = Some(value);
        } else if label.contains("monthly") && label.contains("limit") {
            monthly_limit_usd = Some(value);
        } else if label.contains("monthly") && label.contains("usage") {
            monthly_usage_usd = Some(value);
        }
    }
    Some(OpenCodeGoBilling {
        balance_usd: balance_usd?,
        monthly_limit_usd,
        monthly_usage_usd,
    })
}

fn unix_time_plus_seconds(seconds: f64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    unix_seconds_to_rfc3339(now + seconds.max(0.0) as u64)
}

pub(super) fn unix_seconds_to_rfc3339(seconds: u64) -> String {
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: u64) -> (u64, u64, u64) {
    let z = days_since_epoch as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year as u64, month as u64, day as u64)
}

pub(crate) fn official_quota_rows(
    value: &serde_json::Value,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let buckets = if let Some(buckets) = value
        .get("rateLimitsByLimitId")
        .and_then(serde_json::Value::as_object)
    {
        buckets
            .iter()
            .map(|(id, bucket)| (id.as_str(), bucket))
            .collect::<Vec<_>>()
    } else if let Some(bucket) = value.get("rateLimits") {
        let id = bucket
            .get("limitId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("codex");
        vec![(id, bucket)]
    } else {
        anyhow::bail!("Codex app-server rate-limit response contains no quota buckets");
    };
    let mut rows = Vec::new();
    for (fallback_id, bucket) in buckets {
        let limit_id = bucket
            .get("limitId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .unwrap_or(fallback_id);
        let limit_name = bucket
            .get("limitName")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(limit_id);
        for (window_id, window) in [
            ("primary", bucket.get("primary")),
            ("secondary", bucket.get("secondary")),
        ] {
            let Some(window) = window.filter(|window| !window.is_null()) else {
                continue;
            };
            let used = window
                .get("usedPercent")
                .and_then(json_f64)
                .filter(|used| used.is_finite() && *used >= 0.0)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "official quota {limit_id}.{window_id} has no valid usedPercent"
                    )
                })?;
            let duration_minutes = window
                .get("windowDurationMins")
                .and_then(serde_json::Value::as_u64);
            let window_label = duration_minutes.map_or_else(
                || window_id.to_owned(),
                |minutes| {
                    if minutes % 1_440 == 0 {
                        format!("{}d", minutes / 1_440)
                    } else if minutes % 60 == 0 {
                        format!("{}h", minutes / 60)
                    } else {
                        format!("{minutes}m")
                    }
                },
            );
            let reset_at = window
                .get("resetsAt")
                .and_then(serde_json::Value::as_u64)
                .map(unix_seconds_to_rfc3339);
            rows.push(serde_json::json!({
                "provider_id": "official",
                "provider_display_name": "OpenAI",
                "display_name": format!("OpenAI {limit_name} {window_label}"),
                "quota_id": format!("{limit_id}.{window_id}"),
                "label": format!("{limit_name} · {window_label}"),
                "value": used,
                "used": used,
                "limit": 100.0,
                "remaining": (100.0 - used).max(0.0),
                "error": null,
                "reset_at": reset_at,
                "stale_at": null,
            }));
        }
    }
    if rows.is_empty() {
        anyhow::bail!("Codex app-server rate-limit response contains no quota windows");
    }
    Ok(rows)
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}
