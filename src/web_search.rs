use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use futures_util::{StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::timeout;
use uuid::Uuid;

use crate::anthropic::ModelInfo;
use crate::config::{
    GatewayConfig, ProviderPreset, UpstreamAuthHeader, UpstreamKind, stored_config_path,
};
use crate::sse::SseDecoder;

const CAPABILITY_FILE_VERSION: u64 = 2;
const CAPABILITY_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const PROBE_CONCURRENCY: usize = 4;
const NO_EVIDENCE_PROBE_ATTEMPTS: usize = 3;
const POSITIVE_CONFIRMATION_ATTEMPTS: usize = 1;
const PROBE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);
const RELEASE_REFERENCE_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const PROBE_PROMPT: &str = "Use the web_search server tool now to find the latest OpenAI Codex CLI release tag from https://github.com/openai/codex/releases/latest. Never call codex_mixin_probe_noop. Reply with the release tag only.";

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ModelWebSearchCapability {
    pub model: String,
    pub supported: bool,
    pub evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub probed_at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebSearchProbeSummary {
    pub attempted: usize,
    pub cached: usize,
    pub supported: usize,
    pub unsupported: usize,
    pub failed: usize,
    pub results: Vec<ModelWebSearchCapability>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
struct UpstreamIdentity {
    provider_preset: String,
    upstream_kind: String,
    upstream_base_url: String,
    upstream_messages_path: String,
    web_search_tool_type: String,
}

impl UpstreamIdentity {
    fn from_config(config: &GatewayConfig) -> Self {
        Self {
            provider_preset: config.provider_preset.as_str().to_owned(),
            upstream_kind: config.upstream_kind.as_str().to_owned(),
            upstream_base_url: config.upstream_base_url.clone(),
            upstream_messages_path: config.upstream_messages_path.clone(),
            web_search_tool_type: config.web_search_tool_type.clone(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct CapabilitySnapshot {
    version: u64,
    upstream: UpstreamIdentity,
    models: BTreeMap<String, ModelWebSearchCapability>,
}

#[derive(Clone)]
pub struct WebSearchCapabilities {
    path: Arc<PathBuf>,
    upstream: UpstreamIdentity,
    models: Arc<RwLock<BTreeMap<String, ModelWebSearchCapability>>>,
}

impl WebSearchCapabilities {
    pub fn clear_default_cache() -> anyhow::Result<bool> {
        let path = default_capability_path();
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        Ok(true)
    }

    pub fn from_default_path(config: &GatewayConfig) -> anyhow::Result<Self> {
        Self::load(default_capability_path(), config)
    }

    pub fn load(path: PathBuf, config: &GatewayConfig) -> anyhow::Result<Self> {
        let upstream = UpstreamIdentity::from_config(config);
        let models = if path.exists() {
            let raw =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            let snapshot: CapabilitySnapshot =
                serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
            if snapshot.version != CAPABILITY_FILE_VERSION {
                tracing::info!(
                    path = %path.display(),
                    cached_version = snapshot.version,
                    current_version = CAPABILITY_FILE_VERSION,
                    "discarding incompatible web search capability cache"
                );
                BTreeMap::new()
            } else if snapshot.upstream == upstream {
                snapshot.models
            } else {
                BTreeMap::new()
            }
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            path: Arc::new(path),
            upstream,
            models: Arc::new(RwLock::new(models)),
        })
    }

    pub fn supports_model(&self, model: &str) -> bool {
        let model = model.strip_suffix("-custom").unwrap_or(model);
        let now = unix_seconds().expect("system clock before Unix epoch");
        self.models
            .read()
            .expect("web search capability lock poisoned")
            .get(model)
            .is_some_and(|capability| capability.supported && capability_is_fresh(capability, now))
    }

    pub fn annotate_models(&self, models: &mut [ModelInfo]) {
        let now = unix_seconds().expect("system clock before Unix epoch");
        let capabilities = self
            .models
            .read()
            .expect("web search capability lock poisoned");
        for model in models {
            model.supports_web_search = capabilities
                .get(&model.id)
                .filter(|capability| capability_is_fresh(capability, now))
                .map(|capability| capability.supported);
        }
    }

    pub fn supported_model_ids(&self) -> HashSet<String> {
        let now = unix_seconds().expect("system clock before Unix epoch");
        self.models
            .read()
            .expect("web search capability lock poisoned")
            .values()
            .filter(|capability| capability.supported && capability_is_fresh(capability, now))
            .map(|capability| capability.model.clone())
            .collect()
    }

    pub fn results(&self) -> Vec<ModelWebSearchCapability> {
        self.models
            .read()
            .expect("web search capability lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub async fn probe_models(
        &self,
        models: &mut [ModelInfo],
        config: &GatewayConfig,
        force: bool,
    ) -> anyhow::Result<WebSearchProbeSummary> {
        let now = unix_seconds()?;
        let mut model_ids = models
            .iter()
            .map(|model| model.id.clone())
            .collect::<Vec<_>>();
        model_ids.sort_by_key(|model| model.to_ascii_lowercase());
        model_ids.dedup();
        let current_models = model_ids.iter().cloned().collect::<HashSet<_>>();
        let candidates = {
            let capabilities = self
                .models
                .read()
                .expect("web search capability lock poisoned");
            model_ids
                .iter()
                .filter(|model| {
                    force
                        || capabilities.get(*model).is_none_or(|capability| {
                            capability.error.is_some()
                                || now.saturating_sub(capability.probed_at)
                                    >= CAPABILITY_TTL.as_secs()
                        })
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let pruned = {
            let mut capabilities = self
                .models
                .write()
                .expect("web search capability lock poisoned");
            let previous_len = capabilities.len();
            capabilities.retain(|model, _| current_models.contains(model));
            capabilities.len() != previous_len
        };
        let attempted = candidates.len();
        if !candidates.is_empty() {
            let client = Client::builder().build()?;
            let release_reference = if candidates
                .iter()
                .any(|model| model.to_ascii_lowercase().starts_with("gpt-"))
            {
                match fetch_release_reference(&client).await {
                    Ok(reference) => Some(reference),
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "flattened GPT web search results cannot be verified"
                        );
                        None
                    }
                }
            } else {
                None
            };
            let config = Arc::new(config.clone());
            let client = Arc::new(client);
            let probe_results = stream::iter(candidates.into_iter().map(|model| {
                let client = Arc::clone(&client);
                let config = Arc::clone(&config);
                let release_reference = release_reference.clone();
                async move {
                    let result =
                        probe_model(&client, &config, &model, release_reference.as_deref()).await;
                    match result {
                        Ok((supported, evidence)) => ModelWebSearchCapability {
                            model,
                            supported,
                            evidence,
                            error: None,
                            probed_at: now,
                        },
                        Err(error) => ModelWebSearchCapability {
                            model,
                            supported: false,
                            evidence: "probe_failed".to_owned(),
                            error: Some(error.to_string()),
                            probed_at: now,
                        },
                    }
                }
            }))
            .buffer_unordered(PROBE_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;

            for capability in &probe_results {
                if let Some(error) = &capability.error {
                    tracing::warn!(
                        model = capability.model,
                        error,
                        "web search capability probe failed"
                    );
                } else {
                    tracing::info!(
                        model = capability.model,
                        supported = capability.supported,
                        evidence = capability.evidence,
                        "web search capability probed"
                    );
                }
            }
            {
                let mut capabilities = self
                    .models
                    .write()
                    .expect("web search capability lock poisoned");
                for capability in probe_results {
                    capabilities.insert(capability.model.clone(), capability);
                }
            }
            self.save()?;
        } else if pruned {
            self.save()?;
        }

        self.annotate_models(models);
        let results = self
            .results()
            .into_iter()
            .filter(|capability| current_models.contains(&capability.model))
            .collect::<Vec<_>>();
        Ok(WebSearchProbeSummary {
            attempted,
            cached: results.len().saturating_sub(attempted),
            supported: results
                .iter()
                .filter(|capability| capability.supported && capability.error.is_none())
                .count(),
            unsupported: results
                .iter()
                .filter(|capability| !capability.supported && capability.error.is_none())
                .count(),
            failed: results
                .iter()
                .filter(|capability| capability.error.is_some())
                .count(),
            results,
        })
    }

    fn save(&self) -> anyhow::Result<()> {
        let snapshot = CapabilitySnapshot {
            version: CAPABILITY_FILE_VERSION,
            upstream: self.upstream.clone(),
            models: self
                .models
                .read()
                .expect("web search capability lock poisoned")
                .clone(),
        };
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("web-search-capabilities.json");
        let temporary_path = self
            .path
            .with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .with_context(|| format!("open {}", temporary_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&serde_json::to_vec_pretty(&snapshot)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary_path, self.path.as_ref())
            .with_context(|| format!("replace {}", self.path.display()))
    }
}

async fn fetch_release_reference(client: &Client) -> anyhow::Result<String> {
    let response = client
        .get(RELEASE_REFERENCE_URL)
        .header("user-agent", "codex-mixin-web-search-probe")
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("release reference endpoint returned {status}");
    }
    serde_json::from_str::<Value>(&body)?
        .get("tag_name")
        .and_then(Value::as_str)
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .context("release reference response has no tag_name")
}

async fn probe_model(
    client: &Client,
    config: &GatewayConfig,
    model: &str,
    release_reference: Option<&str>,
) -> anyhow::Result<(bool, String)> {
    let mut last_error = None;
    for _ in 0..NO_EVIDENCE_PROBE_ATTEMPTS {
        let verdict = match timeout(
            PROBE_ATTEMPT_TIMEOUT,
            probe_model_once(client, config, model, release_reference),
        )
        .await
        {
            Ok(Ok(verdict)) => verdict,
            Ok(Err(error)) => {
                last_error = Some(error);
                continue;
            }
            Err(_) => {
                last_error = Some(anyhow::anyhow!(
                    "web search probe attempt timed out after {} seconds",
                    PROBE_ATTEMPT_TIMEOUT.as_secs()
                ));
                continue;
            }
        };
        match verdict {
            ProbeVerdict::Supported(evidence) => {
                for _ in 0..POSITIVE_CONFIRMATION_ATTEMPTS {
                    match timeout(
                        PROBE_ATTEMPT_TIMEOUT,
                        probe_model_once(client, config, model, release_reference),
                    )
                    .await
                    {
                        Ok(Ok(ProbeVerdict::Supported(_))) => {}
                        Ok(Ok(ProbeVerdict::Unsupported(evidence))) => {
                            return Ok((false, evidence.to_owned()));
                        }
                        Ok(Ok(ProbeVerdict::NoEvidence)) => {
                            return Ok((false, "inconsistent_no_search_evidence".to_owned()));
                        }
                        Ok(Err(error)) => {
                            return Err(error).with_context(|| {
                                format!("web search confirmation failed for {model}")
                            });
                        }
                        Err(_) => {
                            anyhow::bail!(
                                "web search confirmation timed out after {} seconds for {model}",
                                PROBE_ATTEMPT_TIMEOUT.as_secs()
                            );
                        }
                    }
                }
                return Ok((true, evidence.to_owned()));
            }
            ProbeVerdict::Unsupported(evidence) => return Ok((false, evidence.to_owned())),
            ProbeVerdict::NoEvidence => {}
        }
    }
    if let Some(error) = last_error {
        return Err(error);
    }
    Ok((false, "no_search_evidence".to_owned()))
}

async fn probe_model_once(
    client: &Client,
    config: &GatewayConfig,
    model: &str,
    release_reference: Option<&str>,
) -> anyhow::Result<ProbeVerdict> {
    if config.upstream_kind == UpstreamKind::OpenAiChat {
        return Ok(ProbeVerdict::Unsupported(
            "openai_chat_adapter_has_no_hosted_search",
        ));
    }
    let mut body = json!({
        "model": model,
        "max_tokens": 512,
        "stream": true,
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": PROBE_PROMPT}]
        }],
        "tool_choice": {"type": "tool", "name": "web_search"},
        "tools": [
            {
                "type": config.web_search_tool_type,
                "name": "web_search",
                "max_uses": 1
            },
            {
                "name": "codex_mixin_probe_noop",
                "description": "Compatibility probe only. Never call this tool.",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            }
        ]
    });
    if config.provider_preset == ProviderPreset::BaiduOneApi {
        body["metadata"] = json!({
            "session_id": format!("web-search-probe-{}", Uuid::new_v4().simple())
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
    let response = request.json(&body).send().await?;
    let status = response.status();
    if !status.is_success() {
        if matches!(
            status,
            reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNPROCESSABLE_ENTITY
        ) {
            return Ok(ProbeVerdict::Unsupported(match status.as_u16() {
                400 => "upstream_rejected_tool_http_400",
                422 => "upstream_rejected_tool_http_422",
                _ => unreachable!("matched only HTTP 400 and 422"),
            }));
        }
        anyhow::bail!("web search probe endpoint returned {status}");
    }

    let mut observation = ProbeObservation::default();
    let mut decoder = SseDecoder::default();
    let mut response_stream = response.bytes_stream();
    while let Some(chunk) = response_stream.next().await {
        for event in decoder.push(&chunk?) {
            let payload: Value = serde_json::from_str(&event.data)
                .context("web search probe returned invalid SSE JSON")?;
            observation.observe(&payload);
            if observation.server_search_result {
                return Ok(ProbeVerdict::Supported("server_tool_result"));
            }
            if observation.ordinary_tool_call {
                return Ok(ProbeVerdict::Unsupported("ordinary_client_tool_call"));
            }
            if let Some(error) = &observation.error {
                anyhow::bail!("web search probe stream failed: {error}");
            }
        }
    }
    if !decoder.remaining().is_empty() {
        let payload: Value = serde_json::from_slice(decoder.remaining())
            .context("web search probe returned neither valid SSE nor JSON")?;
        observation.observe(&payload);
    }
    if observation.server_search_result {
        return Ok(ProbeVerdict::Supported("server_tool_result"));
    }
    if observation.ordinary_tool_call {
        return Ok(ProbeVerdict::Unsupported("ordinary_client_tool_call"));
    }
    if let Some(error) = observation.error {
        anyhow::bail!("web search probe failed: {error}");
    }
    if observation.server_tool_started {
        anyhow::bail!("web search server tool started without returning a result");
    }
    if !model.to_ascii_lowercase().starts_with("gpt-") {
        return Ok(ProbeVerdict::NoEvidence);
    }
    let Some(release_reference) = release_reference else {
        anyhow::bail!(
            "cannot verify flattened web search because release reference is unavailable"
        );
    };
    if response_matches_release(&observation.text, release_reference) {
        return Ok(ProbeVerdict::Supported("verified_flattened_search_result"));
    }
    Ok(ProbeVerdict::NoEvidence)
}

enum ProbeVerdict {
    Supported(&'static str),
    Unsupported(&'static str),
    NoEvidence,
}

#[derive(Default)]
struct ProbeObservation {
    server_tool_started: bool,
    server_search_result: bool,
    ordinary_tool_call: bool,
    text: String,
    error: Option<String>,
}

impl ProbeObservation {
    fn observe(&mut self, payload: &Value) {
        match payload.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                self.observe_content_block(payload.get("content_block").unwrap_or(&Value::Null));
            }
            Some("content_block_delta") => {
                if let Some(text) = payload.pointer("/delta/text").and_then(Value::as_str) {
                    self.text.push_str(text);
                }
            }
            Some("error") => {
                self.error = payload
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .or_else(|| payload.get("message").and_then(Value::as_str))
                    .map(str::to_owned)
                    .or_else(|| Some(payload.to_string()));
            }
            Some("message") | None => {
                if let Some(content) = payload.get("content").and_then(Value::as_array) {
                    for block in content {
                        self.observe_content_block(block);
                    }
                }
                if let Some(error) = payload.pointer("/error/message").and_then(Value::as_str) {
                    self.error = Some(error.to_owned());
                }
            }
            _ => {}
        }
    }

    fn observe_content_block(&mut self, block: &Value) {
        match block.get("type").and_then(Value::as_str) {
            Some("server_tool_use")
                if block.get("name").and_then(Value::as_str) == Some("web_search") =>
            {
                self.server_tool_started = true;
            }
            Some("web_search_tool_result") => self.server_search_result = true,
            Some("tool_use") if block.get("name").and_then(Value::as_str) == Some("web_search") => {
                self.ordinary_tool_call = true;
            }
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    self.text.push_str(text);
                }
            }
            _ => {}
        }
    }
}

fn response_matches_release(text: &str, release_reference: &str) -> bool {
    let text = text.to_ascii_lowercase();
    let release_reference = release_reference.to_ascii_lowercase();
    let bare_version = release_reference
        .strip_prefix("rust-v")
        .or_else(|| release_reference.strip_prefix('v'))
        .unwrap_or(&release_reference);
    text.contains(&release_reference) || text.contains(bare_version)
}

fn capability_is_fresh(capability: &ModelWebSearchCapability, now: u64) -> bool {
    capability.error.is_none()
        && now.saturating_sub(capability.probed_at) < CAPABILITY_TTL.as_secs()
}

fn default_capability_path() -> PathBuf {
    stored_config_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("web-search-capabilities.json")
}

fn unix_seconds() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(upstream_base_url: &str) -> GatewayConfig {
        GatewayConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            provider_preset: ProviderPreset::Custom,
            upstream_kind: UpstreamKind::AnthropicMessages,
            upstream_base_url: upstream_base_url.to_owned(),
            upstream_messages_path: "/v1/messages".to_owned(),
            upstream_models_path: "/v1/models".to_owned(),
            upstream_image_generation_path: None,
            upstream_api_key: "test-key".to_owned(),
            quota_url: None,
            quota_username: None,
            official_responses_url: "https://example.test/responses".to_owned(),
            codex_auth_path: PathBuf::from("/tmp/auth.json"),
            upstream_auth_header: UpstreamAuthHeader::AuthorizationBearer,
            anthropic_version: "2023-06-01".to_owned(),
            anthropic_beta: None,
            gateway_api_key: None,
            accept_codex_oauth: false,
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_secs(30),
            thinking_mode: crate::config::ThinkingMode::Off,
            enable_web_search_tool: true,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            fusion_profiles: Vec::new(),
        }
    }

    #[test]
    fn recognizes_server_and_client_web_search_blocks() {
        let mut server = ProbeObservation::default();
        server.observe(&json!({
            "type": "content_block_start",
            "content_block": {"type":"server_tool_use","name":"web_search"}
        }));
        assert!(server.server_tool_started);
        assert!(!server.server_search_result);
        assert!(!server.ordinary_tool_call);

        server.observe(&json!({
            "type": "content_block_start",
            "content_block": {"type":"web_search_tool_result","tool_use_id":"srvtoolu_1"}
        }));
        assert!(server.server_search_result);

        let mut client = ProbeObservation::default();
        client.observe(&json!({
            "type": "content_block_start",
            "content_block": {"type":"tool_use","name":"web_search"}
        }));
        assert!(!client.server_tool_started);
        assert!(!client.server_search_result);
        assert!(client.ordinary_tool_call);
    }

    #[test]
    fn verifies_flattened_release_answers() {
        assert!(response_matches_release(
            "The latest release is v0.144.5",
            "rust-v0.144.5"
        ));
        assert!(!response_matches_release(
            "The latest release is v0.114.0",
            "rust-v0.144.5"
        ));
    }

    #[tokio::test]
    async fn persists_prunes_and_invalidates_capabilities() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("web-search-capabilities.json");
        let config = test_config("https://one.example");
        let capabilities = WebSearchCapabilities::load(path.clone(), &config).unwrap();
        let capability = |model: &str| ModelWebSearchCapability {
            model: model.to_owned(),
            supported: true,
            evidence: "server_tool_result".to_owned(),
            error: None,
            probed_at: unix_seconds().unwrap(),
        };
        {
            let mut models = capabilities.models.write().unwrap();
            models.insert(
                "Claude Haiku 4.5".to_owned(),
                capability("Claude Haiku 4.5"),
            );
            models.insert("Removed Model".to_owned(), capability("Removed Model"));
        }
        capabilities.save().unwrap();

        let loaded = WebSearchCapabilities::load(path.clone(), &config).unwrap();
        assert!(loaded.supports_model("Claude Haiku 4.5"));
        assert!(loaded.supports_model("Removed Model"));
        let mut current_models = vec![ModelInfo {
            id: "Claude Haiku 4.5".to_owned(),
            ..ModelInfo::default()
        }];
        let summary = loaded
            .probe_models(&mut current_models, &config, false)
            .await
            .unwrap();
        assert_eq!(summary.attempted, 0);
        assert!(!loaded.supports_model("Removed Model"));

        let another_upstream = test_config("https://two.example");
        let invalidated = WebSearchCapabilities::load(path.clone(), &another_upstream).unwrap();
        assert!(!invalidated.supports_model("Claude Haiku 4.5"));
        assert!(invalidated.results().is_empty());

        let old_snapshot = CapabilitySnapshot {
            version: CAPABILITY_FILE_VERSION - 1,
            upstream: UpstreamIdentity::from_config(&config),
            models: BTreeMap::from([(
                "Claude Haiku 4.5".to_owned(),
                capability("Claude Haiku 4.5"),
            )]),
        };
        fs::write(&path, serde_json::to_vec_pretty(&old_snapshot).unwrap()).unwrap();
        let invalidated = WebSearchCapabilities::load(path, &config).unwrap();
        assert!(invalidated.results().is_empty());
    }
}
