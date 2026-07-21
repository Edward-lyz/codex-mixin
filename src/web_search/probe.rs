use super::types::*;
use super::*;

pub(super) async fn fetch_release_reference(client: &Client) -> anyhow::Result<String> {
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

pub(super) async fn probe_model(
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

pub(super) async fn probe_model_once(
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

pub(super) enum ProbeVerdict {
    Supported(&'static str),
    Unsupported(&'static str),
    NoEvidence,
}

#[derive(Default)]
pub(super) struct ProbeObservation {
    pub(super) server_tool_started: bool,
    pub(super) server_search_result: bool,
    pub(super) ordinary_tool_call: bool,
    pub(super) text: String,
    pub(super) error: Option<String>,
}

impl ProbeObservation {
    pub(super) fn observe(&mut self, payload: &Value) {
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

pub(super) fn response_matches_release(text: &str, release_reference: &str) -> bool {
    let text = text.to_ascii_lowercase();
    let release_reference = release_reference.to_ascii_lowercase();
    let bare_version = release_reference
        .strip_prefix("rust-v")
        .or_else(|| release_reference.strip_prefix('v'))
        .unwrap_or(&release_reference);
    text.contains(&release_reference) || text.contains(bare_version)
}
