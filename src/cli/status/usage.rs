use std::time::Duration;

use codex_mixin::config::GatewayConfig;
use serde::{Deserialize, Serialize};

use super::super::runtime::*;

pub(crate) async fn usage(json_output: bool, days: Option<u64>) -> anyhow::Result<()> {
    let runtime =
        load_runtime_metadata()?.ok_or_else(|| anyhow::anyhow!("gateway is not running"))?;
    if !pid_is_running(runtime.pid)? {
        anyhow::bail!("gateway is not running");
    }
    let config = GatewayConfig::from_stored_config()?;
    let url = match days {
        Some(days) => format!("http://{}/v1/usage?days={days}", runtime.bind),
        None => format!("http://{}/v1/usage", runtime.bind),
    };
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
        anyhow::bail!("usage gateway request failed ({status}): {body}");
    }
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
        let cache_hit = row
            .cache_hit_percent
            .map(|percent| format!(", cache hit {percent:.1}%"))
            .unwrap_or_default();
        println!(
            "{}/{}: {} requests, {} uncached input tokens, {} cached tokens, {} cache creation tokens, {} output tokens{cache_hit}",
            row.provider_id,
            row.model_id,
            row.request_count,
            row.input_tokens,
            row.cache_read_tokens,
            row.cache_creation_tokens,
            row.output_tokens
        );
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_json_preserves_provider_timing_fields() {
        let input = serde_json::json!({
            "provider_id": "official",
            "model_id": "gpt-5.6-sol",
            "request_count": 9,
            "input_tokens": 100,
            "cache_read_tokens": 50,
            "cache_creation_tokens": 0,
            "output_tokens": 900,
            "cache_hit_percent": 50.0,
            "average_ttft_ms": 4172.0,
            "output_tps": 87.6
        });
        let row: ProviderTokenUsageRow = serde_json::from_value(input).unwrap();
        let output = serde_json::to_value(row).unwrap();

        assert_eq!(output["average_ttft_ms"], 4172.0);
        assert_eq!(output["output_tps"], 87.6);
    }
}
