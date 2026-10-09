use std::time::Duration;

use codex_mixin::config::GatewayConfig;
use serde_json::Value;

use super::super::runtime::*;

/// Fetches hourly, daily and per-client usage for one provider.
pub(crate) async fn usage_activity(
    json_output: bool,
    provider: &str,
    days: Option<u64>,
) -> anyhow::Result<()> {
    let runtime =
        load_runtime_metadata()?.ok_or_else(|| anyhow::anyhow!("gateway is not running"))?;
    if !pid_is_running(runtime.pid)? {
        anyhow::bail!("gateway is not running");
    }
    let config = GatewayConfig::from_stored_config()?;
    let url = format!("http://{}/v1/usage-activity", runtime.bind);
    let mut query = vec![("provider_id", provider.to_owned())];
    if let Some(days) = days {
        query.push(("days", days.to_string()));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let mut request = client.get(&url).query(&query);
    if let Some(key) = config.gateway_api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("usage activity gateway request failed ({status}): {body}");
    }
    let activity: Value = serde_json::from_str(&body)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&activity)?);
        return Ok(());
    }
    let count = |key: &str| activity[key].as_array().map_or(0, Vec::len);
    println!(
        "{provider}: {} hourly buckets, {} active days, {} clients",
        count("hourly"),
        count("daily"),
        count("clients")
    );
    for client in activity["clients"].as_array().into_iter().flatten() {
        println!(
            "  {}: {} requests, {} output tokens",
            client["client_id"].as_str().unwrap_or("unknown"),
            client["request_count"],
            client["output_tokens"]
        );
    }
    Ok(())
}
