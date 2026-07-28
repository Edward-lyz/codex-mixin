use std::convert::Infallible;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use bytes::Bytes;

use super::manager::{load_snapshot, save_snapshot};
use super::runner::*;
use super::types::BENCHMARK_FILE_VERSION;
use super::*;
use crate::provider::{
    ProviderModelSource, ProviderProtocol, ProviderQuotaParser, ProviderRegistry, ProviderRuntime,
    custom_provider,
};

async fn spawn_benchmark_server(delay: Duration) -> ProviderRuntime {
    spawn_benchmark_server_for("benchmark-provider", delay).await
}

async fn spawn_benchmark_server_for(id: &str, delay: Duration) -> ProviderRuntime {
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
    let mut provider = test_provider(
        format!("http://{address}"),
        ProviderProtocol::AnthropicMessages,
    );
    provider.id = id.to_owned();
    provider.display_name = id.to_owned();
    provider.quota_url = Some(format!("http://{address}/quota"));
    provider.quota_username = Some("benchmark-user".to_owned());
    provider.quota_currency = Some("CNY".to_owned());
    provider.quota_parser = ProviderQuotaParser::BaiduOneApi;
    runtime(provider)
}

async fn spawn_openai_benchmark_server(delay: Duration) -> ProviderRuntime {
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
    let mut provider = test_provider(format!("http://{address}"), ProviderProtocol::OpenAiChat);
    provider.api_path = "/chat/completions".to_owned();
    runtime(provider)
}

async fn spawn_baidu_responses_benchmark_server() -> (ProviderRuntime, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/v1/responses",
            post(
                |State(requests): State<Arc<Mutex<Vec<Value>>>>,
                 headers: HeaderMap,
                 Json(body): Json<Value>| async move {
                    requests.lock().unwrap().push(json!({
                        "body": body,
                        "anthropic_version": headers
                            .get("anthropic-version")
                            .and_then(|value| value.to_str().ok()),
                    }));
                    Body::from(concat!(
                        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n",
                        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"output_tokens\":100}}}\n\n"
                    ))
                },
            ),
        )
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut provider = test_provider(
        format!("http://{address}"),
        ProviderProtocol::AnthropicMessages,
    );
    provider.model_source = ProviderModelSource::BaiduOneApi;
    (runtime(provider), requests)
}

fn test_provider(
    upstream_base_url: String,
    protocol: ProviderProtocol,
) -> crate::provider::ProviderDefinition {
    let mut provider = custom_provider("benchmark-provider", "upstream-key");
    provider.base_url = upstream_base_url;
    provider.protocol = protocol;
    provider
}

fn runtime(provider: crate::provider::ProviderDefinition) -> ProviderRuntime {
    ProviderRegistry::new(vec![provider]).unwrap().providers()[0].clone()
}

#[test]
fn reads_sub2api_total_actual_cost_for_benchmark_deltas() {
    let payload = json!({
        "mode": "unrestricted",
        "remaining": 37.5,
        "usage": {
            "total": {
                "actual_cost": 12.5
            }
        }
    });

    assert_eq!(
        used_quota_from_json(ProviderQuotaParser::Generic, &payload).unwrap(),
        12.5
    );
    assert_eq!(
        used_quota_from_json(
            ProviderQuotaParser::Generic,
            &json!({"data": {"total_used": 8.25}})
        )
        .unwrap(),
        8.25
    );
}

fn target(provider: &ProviderRuntime, model: &str) -> BenchmarkTarget {
    BenchmarkTarget {
        catalog_slug: format!("{model}-{}", provider.id()),
        provider_id: provider.id().to_owned(),
        provider_name: provider.display_name().to_owned(),
        upstream_model_id: model.to_owned(),
        provider: provider.clone(),
    }
}

#[test]
fn benchmark_request_defaults_to_full_test_and_accepts_ttft_only() {
    let full: StartBenchmarkRequest =
        serde_json::from_value(json!({"timeout_seconds": 10})).unwrap();
    assert_eq!(full.target_output_tokens, BENCHMARK_TARGET_OUTPUT_TOKENS);

    let ttft: StartBenchmarkRequest = serde_json::from_value(json!({
        "timeout_seconds": 10,
        "target_output_tokens": 1
    }))
    .unwrap();
    assert_eq!(ttft.target_output_tokens, 1);
}

#[tokio::test]
async fn measures_ttft_and_generation_tps() {
    let provider = spawn_benchmark_server(Duration::from_millis(20)).await;
    let client = Client::new();

    let result = benchmark_model(
        &client,
        &target(&provider, "Claude Sonnet 5"),
        Duration::from_secs(1),
        BENCHMARK_TARGET_OUTPUT_TOKENS,
    )
    .await
    .unwrap();

    assert_eq!(result.status, BenchmarkResultStatus::Completed);
    assert_eq!(result.output_tokens, Some(100));
    assert!(result.ttft_ms.unwrap() >= 15);
    assert!(result.generation_ms.unwrap() >= 15);
    assert!(result.tps.unwrap().is_finite());
}

#[tokio::test]
async fn ttft_only_finishes_at_the_first_token() {
    let provider = spawn_benchmark_server(Duration::from_millis(30)).await;
    let client = Client::new();

    let result = benchmark_model(
        &client,
        &target(&provider, "latency-only"),
        Duration::from_millis(50),
        1,
    )
    .await
    .unwrap();

    assert_eq!(result.status, BenchmarkResultStatus::Completed);
    assert_eq!(result.output_tokens, Some(1));
    assert!(result.ttft_ms.unwrap() >= 25);
    assert!(result.total_ms < 50);
    assert!(result.generation_ms.is_none());
    assert!(result.tps.is_none());
}

#[tokio::test]
async fn records_per_model_timeout() {
    let provider = spawn_benchmark_server(Duration::from_millis(100)).await;
    let client = Client::new();

    let result = benchmark_model(
        &client,
        &target(&provider, "slow-model"),
        Duration::from_millis(20),
        BENCHMARK_TARGET_OUTPUT_TOKENS,
    )
    .await
    .unwrap();

    assert_eq!(result.status, BenchmarkResultStatus::TimedOut);
    assert!(result.error.unwrap().contains("timed out"));
}

#[tokio::test]
async fn measures_openai_reasoning_tokens() {
    let provider = spawn_openai_benchmark_server(Duration::from_millis(20)).await;
    let client = Client::new();

    let result = benchmark_model(
        &client,
        &target(&provider, "deepseek-reasoner"),
        Duration::from_secs(1),
        BENCHMARK_TARGET_OUTPUT_TOKENS,
    )
    .await
    .unwrap();

    assert_eq!(result.status, BenchmarkResultStatus::Completed);
    assert_eq!(result.output_tokens, Some(100));
    assert!(result.ttft_ms.unwrap() >= 15);
    assert!(result.tps.unwrap().is_finite());
}

#[tokio::test]
async fn benchmarks_baidu_gpt_through_responses_protocol() {
    let (provider, requests) = spawn_baidu_responses_benchmark_server().await;

    let result = benchmark_model(
        &Client::new(),
        &target(&provider, "gpt-5.6-sol"),
        Duration::from_secs(1),
        BENCHMARK_TARGET_OUTPUT_TOKENS,
    )
    .await
    .unwrap();

    assert_eq!(result.status, BenchmarkResultStatus::Completed);
    assert_eq!(result.output_tokens, Some(100));
    let request = requests.lock().unwrap()[0].clone();
    assert_eq!(request["body"]["model"], "gpt-5.6-sol");
    assert_eq!(request["body"]["input"], BENCHMARK_PROMPT);
    assert!(request["body"].get("messages").is_none());
    assert!(request["anthropic_version"].is_null());
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
    let provider = runtime(test_provider(
        format!("http://{address}"),
        ProviderProtocol::AnthropicMessages,
    ));

    let result = benchmark_model(
        &Client::new(),
        &target(&provider, "Kimi-K2.7-Code"),
        Duration::from_secs(1),
        BENCHMARK_TARGET_OUTPUT_TOKENS,
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
    let provider = spawn_benchmark_server(Duration::from_millis(5)).await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model-benchmarks.json");
    let manager = ModelBenchmarkManager::new(path.clone());

    manager
        .start(
            vec![target(&provider, "model-a"), target(&provider, "model-b")],
            Duration::from_secs(1),
            BENCHMARK_TARGET_OUTPUT_TOKENS,
        )
        .unwrap();
    for _ in 0..100 {
        let snapshot = manager.snapshot().unwrap().unwrap();
        if snapshot.status == BenchmarkRunStatus::Completed {
            assert_eq!(snapshot.results.len(), 2);
            assert_eq!(snapshot.results[0].model, "model-a-benchmark-provider");
            assert_eq!(snapshot.results[1].model, "model-b-benchmark-provider");
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

#[tokio::test]
async fn runs_different_provider_groups_concurrently() {
    let first = spawn_benchmark_server_for("first-provider", Duration::from_millis(75)).await;
    let second = spawn_benchmark_server_for("second-provider", Duration::from_millis(75)).await;
    let directory = tempfile::tempdir().unwrap();
    let manager = ModelBenchmarkManager::new(directory.path().join("model-benchmarks.json"));
    let started = Instant::now();

    manager
        .start(
            vec![target(&first, "same-model"), target(&second, "same-model")],
            Duration::from_secs(2),
            BENCHMARK_TARGET_OUTPUT_TOKENS,
        )
        .unwrap();
    for _ in 0..100 {
        let snapshot = manager.snapshot().unwrap().unwrap();
        if snapshot.status == BenchmarkRunStatus::Completed {
            assert_eq!(snapshot.results.len(), 2);
            assert!(
                snapshot
                    .results
                    .iter()
                    .all(|result| result.status == BenchmarkResultStatus::Completed)
            );
            assert_eq!(snapshot.provider_costs.len(), 2);
            assert!(
                started.elapsed() < Duration::from_millis(400),
                "provider groups ran sequentially: {:?}",
                started.elapsed()
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("multi-provider benchmark did not finish");
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
        provider_costs: Vec::new(),
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
fn rejects_a_snapshot_from_the_single_provider_schema() {
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

    assert!(
        load_snapshot(&path)
            .unwrap_err()
            .to_string()
            .contains("unsupported")
    );
}
