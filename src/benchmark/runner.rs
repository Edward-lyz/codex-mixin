use super::manager::unix_millis;
use super::types::*;
use super::*;

pub(super) async fn benchmark_model(
    client: &Client,
    config: &GatewayConfig,
    model: &str,
    timeout: Duration,
) -> anyhow::Result<ModelBenchmarkResult> {
    let attempt = benchmark_request(client, config, model, timeout).await;
    let completed_at = unix_millis()?;
    match attempt {
        Ok(metrics) => Ok(ModelBenchmarkResult {
            model: model.to_owned(),
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
            model: model.to_owned(),
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
    config: &GatewayConfig,
) -> anyhow::Result<f64> {
    let quota_url = config
        .quota_url
        .as_ref()
        .context("quota endpoint is not configured")?;
    let mut quota_url = reqwest::Url::parse(quota_url).context("invalid quota endpoint URL")?;
    if !quota_url.query_pairs().any(|(key, _)| key == "username")
        && let Some(username) = &config.quota_username
    {
        quota_url
            .query_pairs_mut()
            .append_pair("username", username);
    }
    let response = client
        .get(quota_url)
        .bearer_auth(&config.upstream_api_key)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("quota endpoint returned {status}: {body}");
    }
    let payload: Value =
        serde_json::from_str(&body).context("quota endpoint returned invalid JSON")?;
    used_quota_from_json(config.provider_preset, &payload)
}

pub(super) fn used_quota_from_json(
    provider: ProviderPreset,
    payload: &Value,
) -> anyhow::Result<f64> {
    if payload.get("success").and_then(Value::as_bool) == Some(false) {
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("quota endpoint reported failure");
        anyhow::bail!("quota endpoint failed: {message}");
    }
    let pointers: &[&str] = match provider {
        ProviderPreset::BaiduOneApi => &["/data/used_quota"],
        ProviderPreset::OpenRouter => &["/data/total_usage"],
        ProviderPreset::Custom | ProviderPreset::DeepSeek => &[
            "/data/used_quota",
            "/data/total_usage",
            "/data/used",
            "/data/spent",
            "/data/cost",
            "/used_quota",
            "/total_usage",
            "/used",
            "/spent",
            "/cost",
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
    client: &Client,
    config: &GatewayConfig,
    model: &str,
    timeout: Duration,
) -> Result<BenchmarkMetrics, BenchmarkAttemptFailure> {
    let started = Instant::now();
    let deadline = started + timeout;
    let mut body = match config.upstream_kind {
        UpstreamKind::AnthropicMessages => json!({
            "model": model,
            "max_tokens": BENCHMARK_TARGET_OUTPUT_TOKENS,
            "stream": true,
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": BENCHMARK_PROMPT}]
            }]
        }),
        UpstreamKind::OpenAiChat => json!({
            "model": model,
            "max_tokens": BENCHMARK_TARGET_OUTPUT_TOKENS,
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": [{"role": "user", "content": BENCHMARK_PROMPT}]
        }),
    };
    if config.provider_preset == ProviderPreset::BaiduOneApi {
        body["metadata"] = json!({
            "session_id": format!("benchmark-{}", Uuid::new_v4().simple())
        });
    }
    let request = client
        .post(config.upstream_messages_url())
        .header("accept", "text/event-stream");
    let request = match config.upstream_auth_header {
        UpstreamAuthHeader::AuthorizationBearer => request.bearer_auth(&config.upstream_api_key),
        UpstreamAuthHeader::XApiKey => request.header("x-api-key", &config.upstream_api_key),
    };
    let mut request = request.header("anthropic-version", &config.anthropic_version);
    if let Some(beta) = &config.anthropic_beta {
        request = request.header("anthropic-beta", beta);
    }
    let request = request.json(&body);
    let response = match timeout_at(deadline, request.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            return Err(attempt_failure(false, error.to_string(), started, None));
        }
        Err(_) => return Err(attempt_failure(true, "request timed out", started, None)),
    };
    let status = response.status();
    if !status.is_success() {
        let body = match timeout_at(deadline, response.text()).await {
            Ok(Ok(body)) => body,
            Ok(Err(error)) => error.to_string(),
            Err(_) => {
                return Err(attempt_failure(
                    true,
                    "request timed out while reading the error response",
                    started,
                    None,
                ));
            }
        };
        return Err(attempt_failure(
            false,
            format!("upstream returned {status}: {body}"),
            started,
            None,
        ));
    }

    let mut first_token_at = None;
    let mut last_token_at = None;
    let mut output_tokens = None;
    let mut openai_finished = false;
    let mut decoder = SseDecoder::default();
    let mut stream = response.bytes_stream();
    loop {
        let chunk = match timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(chunk))) => chunk,
            Ok(Some(Err(error))) => {
                return Err(attempt_failure(
                    false,
                    error.to_string(),
                    started,
                    first_token_at,
                ));
            }
            Ok(None) => {
                if config.upstream_kind == UpstreamKind::OpenAiChat && openai_finished {
                    return finish_metrics(started, first_token_at, last_token_at, output_tokens);
                }
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
        let chunk_received_at = Instant::now();
        for event in decoder.push(&chunk) {
            if config.upstream_kind == UpstreamKind::OpenAiChat && event.data == "[DONE]" {
                return finish_metrics(started, first_token_at, last_token_at, output_tokens);
            }
            let payload: Value = serde_json::from_str(&event.data).map_err(|error| {
                attempt_failure(
                    false,
                    format!("upstream returned invalid SSE JSON: {error}"),
                    started,
                    first_token_at,
                )
            })?;
            match config.upstream_kind {
                UpstreamKind::AnthropicMessages => {
                    match payload.get("type").and_then(Value::as_str) {
                        Some("message_start") => {
                            if let Some(tokens) = payload
                                .pointer("/message/usage/output_tokens")
                                .and_then(Value::as_u64)
                            {
                                output_tokens = Some(tokens);
                            }
                        }
                        Some("content_block_start") => {
                            let content_block =
                                payload.get("content_block").unwrap_or(&Value::Null);
                            let has_content = ["text", "thinking"].iter().any(|field| {
                                content_block
                                    .get(field)
                                    .and_then(Value::as_str)
                                    .is_some_and(|value| !value.is_empty())
                            });
                            if has_content {
                                first_token_at.get_or_insert(chunk_received_at);
                                last_token_at = Some(chunk_received_at);
                            }
                        }
                        Some("content_block_delta") => {
                            let delta = payload.get("delta").unwrap_or(&Value::Null);
                            let has_delta = ["text", "thinking"].iter().any(|field| {
                                delta
                                    .get(field)
                                    .and_then(Value::as_str)
                                    .is_some_and(|value| !value.is_empty())
                            });
                            if has_delta {
                                first_token_at.get_or_insert(chunk_received_at);
                                last_token_at = Some(chunk_received_at);
                            }
                        }
                        Some("message_delta") => {
                            if let Some(tokens) = payload
                                .pointer("/usage/output_tokens")
                                .and_then(Value::as_u64)
                            {
                                output_tokens = Some(tokens);
                            }
                        }
                        Some("message_stop") => {
                            return finish_metrics(
                                started,
                                first_token_at,
                                last_token_at,
                                output_tokens,
                            );
                        }
                        Some("error") => {
                            let message = payload
                                .pointer("/error/message")
                                .and_then(Value::as_str)
                                .unwrap_or("upstream returned an error event");
                            return Err(attempt_failure(false, message, started, first_token_at));
                        }
                        _ => {}
                    }
                }
                UpstreamKind::OpenAiChat => {
                    if let Some(message) = payload.pointer("/error/message").and_then(Value::as_str)
                    {
                        return Err(attempt_failure(false, message, started, first_token_at));
                    }
                    if let Some(usage) = payload.get("usage")
                        && let Some(tokens) = usage.get("completion_tokens").and_then(Value::as_u64)
                    {
                        output_tokens = Some(tokens);
                    }
                    if let Some(choice) = payload
                        .get("choices")
                        .and_then(Value::as_array)
                        .and_then(|choices| choices.first())
                    {
                        let delta = choice.get("delta").unwrap_or(&Value::Null);
                        let has_delta =
                            ["content", "reasoning_content", "reasoning"]
                                .iter()
                                .any(|field| {
                                    delta
                                        .get(field)
                                        .and_then(Value::as_str)
                                        .is_some_and(|value| !value.is_empty())
                                });
                        if has_delta {
                            first_token_at.get_or_insert(chunk_received_at);
                            last_token_at = Some(chunk_received_at);
                        }
                        if choice
                            .get("finish_reason")
                            .and_then(Value::as_str)
                            .is_some()
                        {
                            openai_finished = true;
                        }
                    }
                }
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
