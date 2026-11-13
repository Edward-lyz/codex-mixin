use super::manager::unix_millis;
use super::types::*;
use super::*;

pub(super) async fn benchmark_model(
    gateway: &crate::gateway::GatewayExecutor,
    target: &BenchmarkTarget,
    timeout: Duration,
    target_output_tokens: u64,
) -> anyhow::Result<ModelBenchmarkResult> {
    let attempt = benchmark_request(gateway, target, timeout, target_output_tokens).await;
    let completed_at = unix_millis()?;
    match attempt {
        Ok(metrics) => Ok(ModelBenchmarkResult {
            model: target.catalog_slug.clone(),
            provider_id: target.provider_id.clone(),
            provider_name: target.provider_name.clone(),
            upstream_model: target.upstream_model_id.clone(),
            status: BenchmarkResultStatus::Completed,
            ttft_ms: Some(metrics.ttft_ms),
            generation_ms: metrics.generation_ms,
            total_ms: metrics.total_ms,
            output_tokens: Some(metrics.output_tokens),
            tps: metrics.tps,
            error: None,
            completed_at,
        }),
        Err(failure) => Ok(ModelBenchmarkResult {
            model: target.catalog_slug.clone(),
            provider_id: target.provider_id.clone(),
            provider_name: target.provider_name.clone(),
            upstream_model: target.upstream_model_id.clone(),
            status: if failure.timed_out {
                BenchmarkResultStatus::TimedOut
            } else {
                BenchmarkResultStatus::Failed
            },
            ttft_ms: failure.ttft_ms,
            generation_ms: None,
            total_ms: failure.total_ms,
            output_tokens: None,
            tps: None,
            error: Some(failure.message),
            completed_at,
        }),
    }
}

pub(super) async fn fetch_used_quota(
    client: &Client,
    provider: &ProviderRuntime,
) -> anyhow::Result<f64> {
    let quota_url = provider
        .quota_url()
        .context("quota endpoint is not configured")?;
    let response = provider.apply_auth(client.get(quota_url)).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("quota endpoint returned {status}: {body}");
    }
    let payload: Value =
        serde_json::from_str(&body).context("quota endpoint returned invalid JSON")?;
    used_quota_from_json(provider.quota_parser(), &payload)
}

pub(super) fn used_quota_from_json(
    parser: ProviderQuotaParser,
    payload: &Value,
) -> anyhow::Result<f64> {
    if payload.get("success").and_then(Value::as_bool) == Some(false) {
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("quota endpoint reported failure");
        anyhow::bail!("quota endpoint failed: {message}");
    }
    let pointers: &[&str] = match parser {
        ProviderQuotaParser::BaiduOneApi => &["/data/used_quota"],
        ProviderQuotaParser::OpenRouter => &["/data/total_usage"],
        ProviderQuotaParser::DeepSeek => &[],
        ProviderQuotaParser::OpenCodeGo => &[],
        ProviderQuotaParser::Generic => &[
            "/data/used_quota",
            "/data/total_usage",
            "/data/total_used",
            "/data/used",
            "/data/spent",
            "/data/cost",
            "/data/actual_cost",
            "/usage/total/actual_cost",
            "/data/usage/total/actual_cost",
            "/used_quota",
            "/total_usage",
            "/total_used",
            "/used",
            "/spent",
            "/cost",
            "/actual_cost",
        ],
    };
    let used = pointers.iter().find_map(|pointer| {
        payload.pointer(pointer).and_then(|value| match value {
            Value::Number(number) => number.as_f64(),
            Value::String(number) => number.parse::<f64>().ok(),
            _ => None,
        })
    });
    match used {
        Some(used) if used.is_finite() && used >= 0.0 => Ok(used),
        Some(_) => anyhow::bail!("quota endpoint returned an invalid used amount"),
        None => anyhow::bail!("quota endpoint response does not contain a used amount"),
    }
}

pub(super) async fn benchmark_request(
    gateway: &crate::gateway::GatewayExecutor,
    target: &BenchmarkTarget,
    timeout: Duration,
    target_output_tokens: u64,
) -> Result<BenchmarkMetrics, BenchmarkAttemptFailure> {
    let started = Instant::now();
    let deadline = started + timeout;
    let upstream_output_budget = if target_output_tokens == 1 {
        BENCHMARK_TTFT_UPSTREAM_MAX_TOKENS
    } else {
        target_output_tokens
    };
    // The official backend removes max_output_tokens. A bounded instruction also
    // allows it to finish and report real usage instead of generating endlessly.
    let mut body = json!({
        "model": target.upstream_model_id,
        "max_output_tokens": upstream_output_budget,
        "stream": true,
        "store": false,
        "instructions": "Follow the user's short benchmark request.",
        "input": [{"role": "user", "content": [{"type": "input_text", "text":
            format!("Write exactly {upstream_output_budget} unrelated short lowercase English words separated by spaces, then stop. Do not explain.")}]}]
    });
    let mut headers = reqwest::header::HeaderMap::new();
    let session = Uuid::new_v4().to_string();
    headers.insert(
        "session-id",
        reqwest::header::HeaderValue::from_str(&session)
            .map_err(|error| attempt_failure(false, error.to_string(), started, None))?,
    );
    let routing = crate::gateway::stable_oneapi_routing(&headers, &body)
        .map_err(|error| attempt_failure(false, error.to_string(), started, None))?;
    let send = async {
        if target.provider.is_none() {
            return gateway
                .stream_official(&headers, body)
                .await
                .map(|(stream, _)| stream);
        }
        crate::gateway::stream_provider_response(
            gateway,
            crate::gateway::ProviderResponseRequest {
                body: &mut body,
                catalog_slug: &target.catalog_slug,
                provider_id: &target.provider_id,
                upstream_model_id: &target.upstream_model_id,
                routing: routing.as_ref(),
                downstream_model: None,
                headers: &headers,
                native_messages: None,
                authorized_control_plane: true,
            },
        )
        .await
    };
    let mut stream = match timeout_at(deadline, send).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => return Err(attempt_failure(false, error.to_string(), started, None)),
        Err(_) => return Err(attempt_failure(true, "request timed out", started, None)),
    };
    let mut first_token_at = None;
    let mut last_token_at = None;
    let mut decoder = SseDecoder::default();
    loop {
        let chunk = match timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(chunk))) => chunk,
            Ok(Some(Err(error))) => match error {},
            Ok(None) => {
                return Err(attempt_failure(
                    false,
                    "upstream stream ended without a terminal event",
                    started,
                    first_token_at,
                ));
            }
            Err(_) => {
                return Err(attempt_failure(
                    true,
                    "request timed out",
                    started,
                    first_token_at,
                ));
            }
        };
        let received_at = Instant::now();
        for event in decoder.push(&chunk) {
            let payload: Value = serde_json::from_str(&event.data).map_err(|error| {
                attempt_failure(
                    false,
                    format!("upstream returned invalid SSE JSON: {error}"),
                    started,
                    first_token_at,
                )
            })?;
            match payload.get("type").and_then(Value::as_str) {
                Some(kind) if kind.ends_with(".delta") => {
                    if payload
                        .get("delta")
                        .and_then(Value::as_str)
                        .is_some_and(|delta| !delta.is_empty())
                    {
                        first_token_at.get_or_insert(received_at);
                        last_token_at = Some(received_at);
                    }
                }
                Some("response.completed" | "response.incomplete") => {
                    return finish_metrics(
                        started,
                        first_token_at,
                        last_token_at,
                        payload
                            .pointer("/response/usage/output_tokens")
                            .and_then(Value::as_u64),
                    );
                }
                Some("response.failed" | "error") => {
                    let message = payload
                        .pointer("/response/error/message")
                        .or_else(|| payload.pointer("/error/message"))
                        .and_then(Value::as_str)
                        .unwrap_or("upstream returned an error event");
                    return Err(attempt_failure(false, message, started, first_token_at));
                }
                _ => {}
            }
            if target_output_tokens == 1
                && let Some(first) = first_token_at
            {
                let elapsed = first.duration_since(started).as_millis() as u64;
                return Ok(BenchmarkMetrics {
                    ttft_ms: elapsed,
                    generation_ms: None,
                    total_ms: elapsed,
                    output_tokens: 1,
                    tps: None,
                });
            }
        }
    }
}

pub(super) fn finish_metrics(
    started: Instant,
    first_token_at: Option<Instant>,
    last_token_at: Option<Instant>,
    output_tokens: Option<u64>,
) -> Result<BenchmarkMetrics, BenchmarkAttemptFailure> {
    let completed = Instant::now();
    let first_token_at = first_token_at.ok_or_else(|| {
        attempt_failure(
            false,
            "response completed without an output token",
            started,
            None,
        )
    })?;
    let output_tokens = output_tokens.filter(|tokens| *tokens > 0).ok_or_else(|| {
        attempt_failure(
            false,
            "response completed without output token usage",
            started,
            Some(first_token_at),
        )
    })?;
    let generation = last_token_at
        .and_then(|last| last.checked_duration_since(first_token_at))
        .filter(|duration| !duration.is_zero());
    let total = completed.duration_since(started);
    let tps = match generation {
        Some(generation) if output_tokens >= 2 => {
            Some((output_tokens - 1) as f64 / generation.as_secs_f64())
        }
        _ if !total.is_zero() => Some(output_tokens as f64 / total.as_secs_f64()),
        _ => None,
    };
    Ok(BenchmarkMetrics {
        ttft_ms: first_token_at.duration_since(started).as_millis() as u64,
        generation_ms: generation.map(|duration| duration.as_millis() as u64),
        total_ms: total.as_millis() as u64,
        output_tokens,
        tps,
    })
}

pub(super) fn attempt_failure(
    timed_out: bool,
    message: impl Into<String>,
    started: Instant,
    first_token_at: Option<Instant>,
) -> BenchmarkAttemptFailure {
    BenchmarkAttemptFailure {
        timed_out,
        message: message.into(),
        ttft_ms: first_token_at.map(|first| first.duration_since(started).as_millis() as u64),
        total_ms: started.elapsed().as_millis() as u64,
    }
}
