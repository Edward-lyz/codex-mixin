use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::{Instant, timeout_at};
use uuid::Uuid;

use crate::anthropic::ModelInfo;
use crate::config::{
    GatewayConfig, ProviderPreset, UpstreamAuthHeader, UpstreamKind, stored_config_path,
};
use crate::sse::SseDecoder;

pub const BENCHMARK_TARGET_OUTPUT_TOKENS: u64 = 100;
const BENCHMARK_FILE_VERSION: u64 = 1;
pub(crate) const BENCHMARK_PROMPT: &str = "Generate an endless stream of unrelated lowercase English words separated by single spaces. Do not count, explain, punctuate, repeat a fixed pattern, or conclude. Continue until the server cuts off generation.";

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkRunStatus {
    Running,
    Completed,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkResultStatus {
    Completed,
    TimedOut,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelBenchmarkResult {
    pub model: String,
    pub status: BenchmarkResultStatus,
    pub ttft_ms: Option<u64>,
    pub generation_ms: Option<u64>,
    pub total_ms: u64,
    pub output_tokens: Option<u64>,
    pub tps: Option<f64>,
    pub error: Option<String>,
    pub completed_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelBenchmarkSnapshot {
    pub version: u64,
    pub run_id: String,
    pub status: BenchmarkRunStatus,
    pub started_at: u64,
    pub updated_at: u64,
    pub finished_at: Option<u64>,
    pub timeout_seconds: u64,
    pub target_output_tokens: u64,
    pub total_models: usize,
    pub current_model: Option<String>,
    pub results: Vec<ModelBenchmarkResult>,
    pub error: Option<String>,
    #[serde(default)]
    pub estimated_cost: Option<f64>,
    #[serde(default)]
    pub cost_currency: Option<String>,
    #[serde(default)]
    pub cost_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StartBenchmarkRequest {
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct BenchmarkSnapshotResponse {
    pub snapshot: Option<ModelBenchmarkSnapshot>,
}

#[derive(Clone)]
pub struct ModelBenchmarkManager {
    snapshot_path: Arc<PathBuf>,
    running: Arc<AtomicBool>,
    snapshot_cache: Arc<RwLock<Option<ModelBenchmarkSnapshot>>>,
}

struct RunningReset(Arc<AtomicBool>);

impl Drop for RunningReset {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl ModelBenchmarkManager {
    pub fn new(snapshot_path: PathBuf) -> Self {
        Self {
            snapshot_path: Arc::new(snapshot_path),
            running: Arc::new(AtomicBool::new(false)),
            snapshot_cache: Arc::new(RwLock::new(None)),
        }
    }

    pub fn from_default_path() -> Self {
        Self::new(default_benchmark_path())
    }

    pub fn snapshot(&self) -> anyhow::Result<Option<ModelBenchmarkSnapshot>> {
        let cached = self
            .snapshot_cache
            .read()
            .map_err(|_| anyhow::anyhow!("model benchmark snapshot cache is poisoned"))?
            .clone();
        let (mut snapshot, loaded_from_disk) = match cached {
            Some(snapshot) => (snapshot, false),
            None => {
                let Some(snapshot) = load_snapshot(&self.snapshot_path)? else {
                    return Ok(None);
                };
                (snapshot, true)
            }
        };
        if snapshot.status == BenchmarkRunStatus::Running && !self.running.load(Ordering::Acquire) {
            let now = unix_millis()?;
            snapshot.status = BenchmarkRunStatus::Interrupted;
            snapshot.updated_at = now;
            snapshot.finished_at = Some(now);
            snapshot.current_model = None;
            snapshot.error = Some("gateway stopped before the benchmark completed".to_owned());
            if snapshot.estimated_cost.is_none() && snapshot.cost_error.is_none() {
                snapshot.cost_error =
                    Some("benchmark stopped before cost could be calculated".to_owned());
            }
            self.persist_snapshot(&snapshot)?;
        } else if loaded_from_disk {
            *self
                .snapshot_cache
                .write()
                .map_err(|_| anyhow::anyhow!("model benchmark snapshot cache is poisoned"))? =
                Some(snapshot.clone());
        }
        Ok(Some(snapshot))
    }

    pub fn start(
        &self,
        mut models: Vec<ModelInfo>,
        config: GatewayConfig,
        timeout: Duration,
    ) -> anyhow::Result<ModelBenchmarkSnapshot> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            anyhow::bail!("model benchmark timeout must be between 1 and 300 seconds");
        }
        if models.is_empty() {
            anyhow::bail!("model benchmark requires at least one available model");
        }
        if self.running.swap(true, Ordering::AcqRel) {
            anyhow::bail!("a model benchmark is already running");
        }
        models.sort_by_key(|model| model.id.to_lowercase());
        let now = unix_millis()?;
        let snapshot = ModelBenchmarkSnapshot {
            version: BENCHMARK_FILE_VERSION,
            run_id: Uuid::new_v4().simple().to_string(),
            status: BenchmarkRunStatus::Running,
            started_at: now,
            updated_at: now,
            finished_at: None,
            timeout_seconds: timeout.as_secs(),
            target_output_tokens: BENCHMARK_TARGET_OUTPUT_TOKENS,
            total_models: models.len(),
            current_model: None,
            results: Vec::with_capacity(models.len()),
            error: None,
            estimated_cost: None,
            cost_currency: match config.provider_preset {
                ProviderPreset::BaiduOneApi => Some("CNY".to_owned()),
                ProviderPreset::OpenRouter => Some("USD".to_owned()),
                ProviderPreset::Custom | ProviderPreset::DeepSeek => None,
            },
            cost_error: None,
        };
        if let Err(error) = self.persist_snapshot(&snapshot) {
            self.running.store(false, Ordering::Release);
            return Err(error);
        }

        let manager = self.clone();
        let task_snapshot = snapshot.clone();
        tokio::spawn(async move {
            let _running_reset = RunningReset(Arc::clone(&manager.running));
            if let Err(error) = manager.run(task_snapshot, models, config, timeout).await {
                tracing::error!(error = %error, "model benchmark stopped unexpectedly");
                if let Err(persist_error) = manager.persist_failed_run(error.to_string()) {
                    tracing::error!(
                        error = %persist_error,
                        "failed to persist model benchmark failure"
                    );
                }
            }
        });
        Ok(snapshot)
    }

    async fn run(
        &self,
        mut snapshot: ModelBenchmarkSnapshot,
        models: Vec<ModelInfo>,
        config: GatewayConfig,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let client = Client::builder().build()?;
        let quota_before = if config.quota_url.is_some() {
            match tokio::time::timeout(Duration::from_secs(10), fetch_used_quota(&client, &config))
                .await
            {
                Ok(Ok(used)) => Some(used),
                Ok(Err(error)) => {
                    snapshot.cost_error = Some(error.to_string());
                    None
                }
                Err(_) => {
                    snapshot.cost_error =
                        Some("quota endpoint timed out before benchmark".to_owned());
                    None
                }
            }
        } else {
            snapshot.cost_error = Some("quota endpoint is not configured".to_owned());
            None
        };
        snapshot.updated_at = unix_millis()?;
        self.persist_snapshot(&snapshot)?;

        for model in models {
            snapshot.current_model = Some(model.id.clone());
            snapshot.updated_at = unix_millis()?;
            self.persist_snapshot(&snapshot)?;

            let result = benchmark_model(&client, &config, &model.id, timeout).await?;
            snapshot.results.push(result);
            snapshot.updated_at = unix_millis()?;
            self.persist_snapshot(&snapshot)?;
        }

        if let Some(quota_before) = quota_before {
            match tokio::time::timeout(Duration::from_secs(10), fetch_used_quota(&client, &config))
                .await
            {
                Ok(Ok(quota_after)) if quota_after >= quota_before => {
                    snapshot.estimated_cost = Some(quota_after - quota_before);
                    snapshot.cost_error = None;
                }
                Ok(Ok(_)) => {
                    snapshot.cost_error =
                        Some("used quota decreased while benchmark was running".to_owned());
                }
                Ok(Err(error)) => snapshot.cost_error = Some(error.to_string()),
                Err(_) => {
                    snapshot.cost_error =
                        Some("quota endpoint timed out after benchmark".to_owned());
                }
            }
        }

        let now = unix_millis()?;
        snapshot.status = BenchmarkRunStatus::Completed;
        snapshot.updated_at = now;
        snapshot.finished_at = Some(now);
        snapshot.current_model = None;
        self.persist_snapshot(&snapshot)
    }

    fn persist_failed_run(&self, message: String) -> anyhow::Result<()> {
        let Some(mut snapshot) = self.snapshot()? else {
            anyhow::bail!("model benchmark snapshot disappeared while the run was active");
        };
        let now = unix_millis()?;
        snapshot.status = BenchmarkRunStatus::Failed;
        snapshot.updated_at = now;
        snapshot.finished_at = Some(now);
        snapshot.current_model = None;
        snapshot.error = Some(message);
        if snapshot.estimated_cost.is_none() && snapshot.cost_error.is_none() {
            snapshot.cost_error =
                Some("benchmark failed before cost could be calculated".to_owned());
        }
        self.persist_snapshot(&snapshot)
    }

    fn persist_snapshot(&self, snapshot: &ModelBenchmarkSnapshot) -> anyhow::Result<()> {
        save_snapshot(&self.snapshot_path, snapshot)?;
        *self
            .snapshot_cache
            .write()
            .map_err(|_| anyhow::anyhow!("model benchmark snapshot cache is poisoned"))? =
            Some(snapshot.clone());
        Ok(())
    }
}

struct BenchmarkMetrics {
    ttft_ms: u64,
    generation_ms: Option<u64>,
    total_ms: u64,
    output_tokens: u64,
    tps: Option<f64>,
}

struct BenchmarkAttemptFailure {
    timed_out: bool,
    message: String,
    ttft_ms: Option<u64>,
    total_ms: u64,
}

async fn benchmark_model(
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

async fn fetch_used_quota(client: &Client, config: &GatewayConfig) -> anyhow::Result<f64> {
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

fn used_quota_from_json(provider: ProviderPreset, payload: &Value) -> anyhow::Result<f64> {
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

async fn benchmark_request(
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

fn finish_metrics(
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

fn attempt_failure(
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

pub fn default_benchmark_path() -> PathBuf {
    stored_config_path().with_file_name("model-benchmarks.json")
}

pub fn load_snapshot(path: &Path) -> anyhow::Result<Option<ModelBenchmarkSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let snapshot: ModelBenchmarkSnapshot =
        serde_json::from_slice(&contents).with_context(|| format!("parse {}", path.display()))?;
    if snapshot.version != BENCHMARK_FILE_VERSION {
        anyhow::bail!(
            "unsupported model benchmark file version {} in {}",
            snapshot.version,
            path.display()
        );
    }
    Ok(Some(snapshot))
}

fn save_snapshot(path: &Path, snapshot: &ModelBenchmarkSnapshot) -> anyhow::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow::anyhow!("benchmark result path has no parent: {}", path.display())
    })?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid benchmark result filename: {}", path.display()))?;
    let temporary_path =
        path.with_file_name(format!("{file_name}.tmp.{}", Uuid::new_v4().simple()));
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer_pretty(&mut file, snapshot)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary_path, path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result.with_context(|| format!("write {}", path.display()))
}

fn unix_millis() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::atomic::AtomicUsize;

    use axum::Router;
    use axum::body::Body;
    use axum::routing::{get, post};
    use bytes::Bytes;

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
}
