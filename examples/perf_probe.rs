use std::time::{Duration, Instant};

use bytes::Bytes;
use codex_mixin::config::{GatewayConfig, ThinkingMode};
use codex_mixin::convert::{ToolNameMap, responses_to_anthropic};
use codex_mixin::provider::custom_provider;
use codex_mixin::sse::SseDecoder;
use futures_util::StreamExt;
use serde_json::{Value, json};

fn config() -> GatewayConfig {
    let mut provider = custom_provider("perf", "perf-key");
    provider.base_url = "https://perf.example".to_owned();
    provider.cached_models = vec![codex_mixin::provider::ProviderModel {
        id: "DeepSeek-V4-Flash".to_owned(),
        ..codex_mixin::provider::ProviderModel::default()
    }];
    provider.selected_models = vec!["DeepSeek-V4-Flash".to_owned()];
    GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        codex_auth_path: "/tmp/codex-mixin-perf-auth.json".into(),
        gateway_api_key: Some("perf-key".to_owned()),
        gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys::default(),
        accept_codex_oauth: true,
        official_selected_models: None,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(60),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    }
}

fn anthropic_sse(event_count: usize) -> String {
    let mut events = String::with_capacity(event_count * 96);
    events.push_str(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n",
    );
    events.push_str(
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    );
    for index in 0..event_count {
        events.push_str("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"token " );
        events.push_str(&index.to_string());
        events.push_str("\"}}\n\n");
    }
    events.push_str(
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
         event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n\
         event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    events
}

fn large_request() -> Value {
    let mut input = Vec::new();
    for index in 0..200 {
        input.push(json!({
            "type": "message",
            "role": if index % 2 == 0 { "user" } else { "assistant" },
            "content": [{
                "type": "input_text",
                "text": format!("turn {index}: {}", "x".repeat(512))
            }]
        }));
    }
    json!({
        "model": "DeepSeek-V4-Flash",
        "stream": true,
        "instructions": "You are Codex.",
        "input": input,
        "tools": [
            {"type": "function", "name": "exec_command", "description": "run", "parameters": {"type": "object"}}
        ]
    })
}

fn main() {
    let event_count = 50_000;
    let sse = anthropic_sse(event_count);
    let bytes = sse.as_bytes();

    let started = Instant::now();
    let mut decoder = SseDecoder::default();
    let parsed = decoder.push(bytes);
    let decoder_elapsed = started.elapsed();
    println!(
        "sse_decode: {} events in {:?} ({:.0} events/s)",
        parsed.len(),
        decoder_elapsed,
        parsed.len() as f64 / decoder_elapsed.as_secs_f64()
    );

    let started = Instant::now();
    let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(sse.clone()))]);
    let output = codex_mixin::openai_events::map_anthropic_sse(
        upstream,
        json!({"model":"DeepSeek-V4-Flash","tools":[]}),
        ToolNameMap::default(),
    )
    .collect::<Vec<_>>();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let chunks = runtime.block_on(output);
    let mapper_elapsed = started.elapsed();
    let bytes_out = chunks
        .into_iter()
        .map(|chunk| chunk.unwrap().len())
        .sum::<usize>();
    println!(
        "anthropic_mapper: {} events -> {} bytes in {:?} ({:.0} events/s, {:.2} MiB/s)",
        event_count,
        bytes_out,
        mapper_elapsed,
        event_count as f64 / mapper_elapsed.as_secs_f64(),
        bytes_out as f64 / (1024.0 * 1024.0) / mapper_elapsed.as_secs_f64()
    );

    let request = large_request();
    let config = config();
    let started = Instant::now();
    let iterations = 2_000;
    for _ in 0..iterations {
        std::hint::black_box(
            responses_to_anthropic(&request, &config).expect("valid perf request"),
        );
    }
    let convert_elapsed = started.elapsed();
    println!(
        "responses_to_anthropic: {} conversions in {:?} ({:.0} req/s)",
        iterations,
        convert_elapsed,
        iterations as f64 / convert_elapsed.as_secs_f64()
    );
}
