use std::time::Duration;

use codex_mixin::config::GatewayConfig;
use serde::{Deserialize, Serialize};

use super::super::runtime::*;

pub(crate) async fn request_usage(
    json_output: bool,
    limit: Option<u64>,
    before: Option<i64>,
) -> anyhow::Result<()> {
    let runtime =
        load_runtime_metadata()?.ok_or_else(|| anyhow::anyhow!("gateway is not running"))?;
    if !pid_is_running(runtime.pid)? {
        anyhow::bail!("gateway is not running");
    }
    let config = GatewayConfig::from_stored_config()?;
    let mut url = format!("http://{}/v1/request-usage", runtime.bind);
    let mut query = Vec::new();
    if let Some(limit) = limit {
        query.push(format!("limit={limit}"));
    }
    if let Some(before) = before {
        query.push(format!("before={before}"));
    }
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let mut request = client.get(&url);
    if let Some(key) = config.gateway_api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("request usage gateway request failed ({status}): {body}");
    }
    let rows: Vec<ProviderRequestUsageRow> = serde_json::from_str(&body)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no request usage recorded");
        return Ok(());
    }
    for row in rows {
        let ttft = row
            .ttft_micros
            .map(|micros| format!(", ttft {:.0}ms", micros as f64 / 1_000.0))
            .unwrap_or_default();
        println!(
            "{} {} via {}/{}: {} input, {} cached, {} output{ttft}",
            row.client_id,
            row.recorded_at_ms,
            row.provider_id,
            row.model_id,
            row.input_tokens,
            row.cache_read_tokens,
            row.output_tokens
        );
    }
    Ok(())
}

#[derive(Deserialize, Serialize)]
struct ProviderRequestUsageRow {
    id: i64,
    recorded_at_ms: u64,
    client_id: String,
    session_id: Option<String>,
    provider_id: String,
    model_id: String,
    input_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    output_tokens: u64,
    ttft_micros: Option<u64>,
    generation_micros: Option<u64>,
    prefix_state: Option<String>,
    changed_regions: Option<String>,
    reused_turns: u64,
    total_turns: u64,
}
