use std::time::Duration;

use anyhow::Context;
use codex_mixin::anthropic::MessageRequest;
use codex_mixin::config::{GatewayConfig, ThinkingMode};
use codex_mixin::server::AppState;
use futures_util::StreamExt;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    let config = GatewayConfig::from_stored_config().context("load stored config")?;
    let gateway_api_key = config.gateway_api_key.clone();
    let stored = config
        .providers
        .iter()
        .find(|candidate| candidate.id == "baidu-oneapi")
        .context("baidu-oneapi provider missing")?;
    let provider = stored.clone();
    let state = AppState::new(GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: config.official_responses_url.clone(),
        codex_auth_path: config.codex_auth_path.clone(),
        gateway_api_key: gateway_api_key.clone(),
        accept_codex_oauth: true,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(60),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    })?;
    let request = MessageRequest {
        model: "GLM-5.2".to_owned(),
        max_tokens: 32,
        stream: true,
        speed: None,
        messages: vec![codex_mixin::anthropic::Message {
            role: "user".to_owned(),
            content: vec![codex_mixin::anthropic::ContentBlock::Text {
                text: "Reply with hi only.".to_owned(),
            }],
        }],
        system: None,
        tools: Vec::new(),
        tool_choice: None,
        thinking: None,
        output_config: None,
        metadata: None,
    };
    let stream = state
        .send_anthropic_request(
            state.provider("baidu-oneapi").unwrap(),
            &request,
            Some("codex-mixin-e2e-probe"),
        )
        .await?;
    let mut stream = stream;
    let mut saw_delta = false;
    let mut saw_completed = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(anyhow::Error::from)?;
        let text = String::from_utf8_lossy(&chunk);
        if text.contains("content_block_delta") {
            saw_delta = true;
        }
        if text.contains("message_stop") {
            saw_completed = true;
        }
    }
    println!("saw_delta={saw_delta} saw_completed={saw_completed}");
    Ok(())
}
