use std::time::Duration;

use codex_mixin::config::GatewayConfig;
use serde::{Deserialize, Serialize};

use super::super::runtime::*;

pub(crate) async fn usage(json_output: bool, days: Option<u64>) -> anyhow::Result<()> {
    let query = days.map(|days| format!("days={days}")).unwrap_or_default();
    let body = gateway_get("/v1/usage", &query).await?;
    let rows: Vec<ProviderTokenUsageRow> = serde_json::from_str(&body)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no provider token usage recorded");
        return Ok(());
    }
    for row in rows {
        let [requests, input, cached, creation, output] = [
            row.request_count,
            row.input_tokens,
            row.cache_read_tokens,
            row.cache_creation_tokens,
            row.output_tokens,
        ]
        .map(|count| {
            super::super::COUNT_UNITS
                .iter()
                .find(|(scale, _)| count >= *scale - *scale / 20_000)
                .map_or_else(
                    || count.to_string(),
                    |(scale, suffix)| format!("{:.1}{suffix}", count as f64 / *scale as f64),
                )
        });
        let cache_hit = row
            .cache_hit_percent
            .map(|percent| format!(", cache hit {percent:.1}%"))
            .unwrap_or_default();
        println!(
            "{}/{}: {} requests, {} uncached input tokens, {} cached tokens, {} cache creation tokens, {} output tokens{cache_hit}",
            row.provider_id, row.model_id, requests, input, cached, creation, output
        );
    }
    Ok(())
}

/// GETs a usage endpoint from the running gateway and returns the raw body.
async fn gateway_get(path: &str, query: &str) -> anyhow::Result<String> {
    let runtime =
        load_runtime_metadata()?.ok_or_else(|| anyhow::anyhow!("gateway is not running"))?;
    if !pid_is_running(runtime.pid)? {
        anyhow::bail!("gateway is not running");
    }
    let config = GatewayConfig::from_stored_config()?;
    let address = codex_mixin::config::client_address(runtime.bind);
    let mut url = reqwest::Url::parse(&format!("http://{address}{path}"))?;
    if !query.is_empty() {
        url.set_query(Some(query));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let mut request = client.get(url);
    if let Some(key) = config.gateway_api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("usage gateway request failed ({status}): {body}");
    }
    Ok(body)
}

pub(crate) async fn usage_daily(json_output: bool, days: Option<u64>) -> anyhow::Result<()> {
    let query = days.map(|days| format!("days={days}")).unwrap_or_default();
    let body = gateway_get("/v1/usage/daily", &query).await?;
    let rows: Vec<serde_json::Value> = serde_json::from_str(&body)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no provider token usage recorded");
        return Ok(());
    }
    for row in rows {
        let day = row["day"]
            .as_i64()
            .and_then(|day| chrono::DateTime::from_timestamp(day * 86_400, 0))
            .map_or("?".to_owned(), |date| date.format("%Y-%m-%d").to_string());
        let [requests, input, cached, output] = [
            "request_count",
            "input_tokens",
            "cache_read_tokens",
            "output_tokens",
        ]
        .map(|key| {
            row[key].as_u64().map_or_else(
                || row[key].to_string(),
                |count| {
                    super::super::COUNT_UNITS
                        .iter()
                        .find(|(scale, _)| count >= *scale - *scale / 20_000)
                        .map_or_else(
                            || count.to_string(),
                            |(scale, suffix)| {
                                format!("{:.1}{suffix}", count as f64 / *scale as f64)
                            },
                        )
                },
            )
        });
        println!(
            "{day} {}/{}: {} requests, {} input, {} cached, {} output",
            row["provider_id"].as_str().unwrap_or("?"),
            row["model_id"].as_str().unwrap_or("?"),
            requests,
            input,
            cached,
            output,
        );
    }
    Ok(())
}

pub(crate) async fn usage_turns(
    json_output: bool,
    days: Option<u64>,
    provider: Option<&str>,
    limit: Option<usize>,
) -> anyhow::Result<()> {
    let mut query = reqwest::Url::parse("http://query.invalid/")?;
    {
        let mut pairs = query.query_pairs_mut();
        if let Some(days) = days {
            pairs.append_pair("days", &days.to_string());
        }
        if let Some(provider) = provider {
            pairs.append_pair("provider", provider);
        }
        if let Some(limit) = limit {
            pairs.append_pair("limit", &limit.to_string());
        }
    }
    let body = gateway_get("/v1/usage/turns", query.query().unwrap_or("")).await?;
    let rows: Vec<serde_json::Value> = serde_json::from_str(&body)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no turns recorded");
        return Ok(());
    }
    for row in rows {
        println!("{}", turn_line(&row));
    }
    Ok(())
}

fn turn_line(row: &serde_json::Value) -> String {
    let text = |key: &str| row[key].as_str().unwrap_or("?").to_owned();
    let count = |key: &str| {
        row[key].as_u64().map_or_else(
            || "-".to_owned(),
            |count| {
                super::super::COUNT_UNITS
                    .iter()
                    .find(|(scale, _)| count >= *scale - *scale / 20_000)
                    .map_or_else(
                        || count.to_string(),
                        |(scale, suffix)| format!("{:.1}{suffix}", count as f64 / *scale as f64),
                    )
            },
        )
    };
    let cache = row["cache_hit_percent"]
        .as_f64()
        .map_or("-".to_owned(), |percent| format!("{percent:.1}%"));
    let model = match (
        row["returned_model"].as_str(),
        row["model_matches"].as_bool(),
    ) {
        (Some(returned), Some(false)) => format!(" (returned {returned})"),
        _ => String::new(),
    };
    format!(
        "{} {}/{}{model}: input {}, cached {} ({cache}), output {}, reasoning {}",
        row["session_display"]
            .as_str()
            .unwrap_or_else(|| row["session"].as_str().unwrap_or("?")),
        text("provider_id"),
        text("upstream_model_id"),
        count("input_tokens"),
        count("cache_read_tokens"),
        count("output_tokens"),
        count("reasoning_tokens"),
    )
}

#[derive(Deserialize, Serialize)]
struct ProviderTokenUsageRow {
    provider_id: String,
    model_id: String,
    request_count: u64,
    input_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    output_tokens: u64,
    cache_hit_percent: Option<f64>,
    average_ttft_ms: Option<f64>,
    output_tps: Option<f64>,
}
