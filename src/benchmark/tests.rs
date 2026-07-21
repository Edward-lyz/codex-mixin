use std::convert::Infallible;
use std::sync::atomic::AtomicUsize;

use axum::Router;
use axum::body::Body;
use axum::routing::{get, post};
use bytes::Bytes;

use super::manager::{load_snapshot, save_snapshot};
use super::runner::*;
use super::types::BENCHMARK_FILE_VERSION;
use super::*;
use crate::config::ThinkingMode;

async fn spawn_benchmark_server(delay: Duration) -> GatewayConfig {
    let quota_calls = Arc::new(AtomicUsize::new(0));
    let quota_counter = Arc::clone(&quota_calls);
    let app = Router::new()
        .route(
            "/v1/messages",
            post(move || async move {
                let stream = async_stream::stream! {
                    yield Ok::<_, Infallible>(Bytes::from(
                        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"output_tokens\":0}}}\n\n"
                    ));
                    tokio::time::sleep(delay).await;
                    yield Ok::<_, Infallible>(Bytes::from(
                        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"x\"}}\n\n"
                    ));
                    tokio::time::sleep(delay).await;
                    yield Ok::<_, Infallible>(Bytes::from(
                        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"y\"}}\n\n"
                    ));
                    tokio::time::sleep(delay).await;
                    yield Ok::<_, Infallible>(Bytes::from(
                        concat!(
                            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":100},\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n",
                            "data: {\"type\":\"message_stop\"}\n\n"
                        )
                    ));
                };
                Body::from_stream(stream)
            }),
        )
        .route(
            "/quota",
            get(move || {
                let used = if quota_counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    10.0
                } else {
                    10.25
                };
                async move { axum::Json(json!({"data":{"used_quota":used}})) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut config = test_config(format!("http://{address}"));
    config.quota_url = Some(format!("http://{address}/quota"));
    config
}

async fn spawn_openai_benchmark_server(delay: Duration) -> GatewayConfig {
    let app = Router::new().route(
        "/chat/completions",
        post(move || async move {
            let stream = async_stream::stream! {
                tokio::time::sleep(delay).await;
                yield Ok::<_, Infallible>(Bytes::from(
                    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"x\"},\"finish_reason\":null}]}\n\n"
                ));
                tokio::time::sleep(delay).await;
                yield Ok::<_, Infallible>(Bytes::from(
                    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"y\"},\"finish_reason\":null}]}\n\n"
                ));
                tokio::time::sleep(delay).await;
                yield Ok::<_, Infallible>(Bytes::from(concat!(
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"completion_tokens\":100}}\n\n",
                    "data: [DONE]\n\n"
                )));
            };
            Body::from_stream(stream)
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut config = test_config(format!("http://{address}"));
    config.upstream_kind = UpstreamKind::OpenAiChat;
    config.upstream_messages_path = "/chat/completions".to_owned();
    config
}

fn test_config(upstream_base_url: String) -> GatewayConfig {
    GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        provider_preset: ProviderPreset::Custom,
        upstream_kind: UpstreamKind::AnthropicMessages,
        upstream_base_url,
        upstream_messages_path: "/v1/messages".to_owned(),
        upstream_models_path: "/v1/models".to_owned(),
        upstream_image_generation_path: None,
        upstream_api_key: "upstream-key".to_owned(),
        quota_url: None,
        quota_username: None,
        official_responses_url: "https://example.invalid/responses".to_owned(),
        codex_auth_path: PathBuf::from("/tmp/codex-auth.json"),
        upstream_auth_header: UpstreamAuthHeader::AuthorizationBearer,
        anthropic_version: "2023-06-01".to_owned(),
        anthropic_beta: None,
        gateway_api_key: None,
        accept_codex_oauth: true,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(2),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    }
}

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_owned(),
        ..Default::default()
    }
}

#[tokio::test]
async fn measures_ttft_and_generation_tps() {
    let config = spawn_benchmark_server(Duration::from_millis(20)).await;
    let client = Client::new();

    let result = benchmark_model(&client, &config, "Claude Sonnet 5", Duration::from_secs(1))
        .await
        .unwrap();

    assert_eq!(result.status, BenchmarkResultStatus::Completed);
    assert_eq!(result.output_tokens, Some(100));
    assert!(result.ttft_ms.unwrap() >= 15);
    assert!(result.generation_ms.unwrap() >= 15);
    assert!(result.tps.unwrap().is_finite());
}

#[tokio::test]
async fn records_per_model_timeout() {
    let config = spawn_benchmark_server(Duration::from_millis(100)).await;
    let client = Client::new();

    let result = benchmark_model(&client, &config, "slow-model", Duration::from_millis(20))
        .await
        .unwrap();

    assert_eq!(result.status, BenchmarkResultStatus::TimedOut);
    assert!(result.error.unwrap().contains("timed out"));
}

#[tokio::test]
async fn measures_openai_reasoning_tokens() {
    let config = spawn_openai_benchmark_server(Duration::from_millis(20)).await;
    let client = Client::new();

    let result = benchmark_model(
        &client,
        &config,
        "deepseek-reasoner",
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    assert_eq!(result.status, BenchmarkResultStatus::Completed);
    assert_eq!(result.output_tokens, Some(100));
    assert!(result.ttft_ms.unwrap() >= 15);
    assert!(result.tps.unwrap().is_finite());
}

#[tokio::test]
async fn uses_end_to_end_tps_when_all_output_arrives_in_one_network_chunk() {
    let app = Router::new().route(
        "/v1/messages",
        post(|| async {
            tokio::time::sleep(Duration::from_millis(40)).await;
            Body::from(concat!(
                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"output_tokens\":0}}}\n\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"all output in one chunk\"}}\n\n",
                "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":100},\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n"
            ))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = test_config(format!("http://{address}"));

    let result = benchmark_model(
        &Client::new(),
        &config,
        "Kimi-K2.7-Code",
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    assert_eq!(result.status, BenchmarkResultStatus::Completed);
    assert_eq!(result.output_tokens, Some(100));
    assert!(result.generation_ms.is_none());
    let expected_tps = 100.0 / (result.total_ms as f64 / 1_000.0);
    let measured_tps = result.tps.unwrap();
    assert!((measured_tps - expected_tps).abs() / expected_tps < 0.05);
}

#[tokio::test]
async fn persists_each_result_and_finishes_the_run() {
    let mut config = spawn_benchmark_server(Duration::from_millis(5)).await;
    config.provider_preset = ProviderPreset::BaiduOneApi;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model-benchmarks.json");
    let manager = ModelBenchmarkManager::new(path.clone());

    manager
        .start(
            vec![model("model-b"), model("model-a")],
            config,
            Duration::from_secs(1),
        )
        .unwrap();
    for _ in 0..100 {
        let snapshot = manager.snapshot().unwrap().unwrap();
        if snapshot.status == BenchmarkRunStatus::Completed {
            assert_eq!(snapshot.results.len(), 2);
            assert_eq!(snapshot.results[0].model, "model-a");
            assert_eq!(snapshot.results[1].model, "model-b");
            assert_eq!(snapshot.estimated_cost, Some(0.25));
            assert_eq!(snapshot.cost_currency.as_deref(), Some("CNY"));
            assert!(snapshot.cost_error.is_none());
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
            fs::remove_file(&path).unwrap();
            assert_eq!(manager.snapshot().unwrap().unwrap().run_id, snapshot.run_id);
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("benchmark did not finish");
}

#[test]
fn marks_an_unfinished_run_interrupted_after_gateway_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model-benchmarks.json");
    let snapshot = ModelBenchmarkSnapshot {
        version: BENCHMARK_FILE_VERSION,
        run_id: "stale-run".to_owned(),
        status: BenchmarkRunStatus::Running,
        started_at: 1,
        updated_at: 1,
        finished_at: None,
        timeout_seconds: 10,
        target_output_tokens: BENCHMARK_TARGET_OUTPUT_TOKENS,
        total_models: 2,
        current_model: Some("model-b".to_owned()),
        results: Vec::new(),
        error: None,
        estimated_cost: None,
        cost_currency: None,
        cost_error: None,
    };
    save_snapshot(&path, &snapshot).unwrap();

    let snapshot = ModelBenchmarkManager::new(path)
        .snapshot()
        .unwrap()
        .unwrap();

    assert_eq!(snapshot.status, BenchmarkRunStatus::Interrupted);
    assert!(snapshot.finished_at.is_some());
    assert!(snapshot.current_model.is_none());
    assert!(snapshot.error.unwrap().contains("gateway stopped"));
}

#[test]
fn loads_a_completed_snapshot_without_new_cost_fields() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model-benchmarks.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "version": 1,
            "run_id": "previous-version",
            "status": "completed",
            "started_at": 1,
            "updated_at": 2,
            "finished_at": 2,
            "timeout_seconds": 10,
            "target_output_tokens": 100,
            "total_models": 1,
            "current_model": null,
            "results": [],
            "error": null
        }))
        .unwrap(),
    )
    .unwrap();

    let snapshot = load_snapshot(&path).unwrap().unwrap();

    assert_eq!(snapshot.run_id, "previous-version");
    assert!(snapshot.estimated_cost.is_none());
    assert!(snapshot.cost_currency.is_none());
    assert!(snapshot.cost_error.is_none());
}
