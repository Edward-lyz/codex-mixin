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
    provider: &ProviderRuntime,
    upstream_model: &str,
    web_search_tool_type: &str,
    release_reference: Option<&str>,
) -> anyhow::Result<(bool, String)> {
    let mut last_error = None;
    for _ in 0..NO_EVIDENCE_PROBE_ATTEMPTS {
        let verdict = match timeout(
            PROBE_ATTEMPT_TIMEOUT,
            probe_model_once(
                client,
                provider,
                upstream_model,
                web_search_tool_type,
                release_reference,
            ),
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
    provider: &ProviderRuntime,
    upstream_model: &str,
    web_search_tool_type: &str,
    release_reference: Option<&str>,
) -> anyhow::Result<ProbeVerdict> {
    if provider.protocol() != ProviderProtocol::AnthropicMessages {
        return Ok(ProbeVerdict::Unsupported(
            "provider_protocol_has_no_anthropic_hosted_search",
        ));
    }
    let mut body = json!({
        "model": upstream_model,
        "max_tokens": 512,
        "stream": true,
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": PROBE_PROMPT}]
        }],
        "tool_choice": {"type": "tool", "name": "web_search"},
        "tools": [
            {
                "type": web_search_tool_type,
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
    if provider.uses_session_affinity() {
        body["metadata"] = json!({
            "session_id": format!("web-search-probe-{}", uuid::Uuid::new_v4().simple())
        });
    }
    let native_headers = if provider.is_baidu_model_source() && provider.uses_ducx_loopback() {
        Some(crate::provider::native_baidu_headers(provider).await?)
    } else {
        None
    };
    let base = client
        .post(provider.api_url().clone())
        .header("accept", "text/event-stream");
    let request = match &native_headers {
        Some(headers) => base.headers(headers.clone()),
        None => provider.apply_auth(base),
    };
    let request =
        provider.apply_anthropic_beta(request, provider.definition().anthropic_beta.as_deref());
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
    let mut raw_response = Vec::new();
    while let Some(chunk) = response_stream.next().await {
        let chunk = chunk?;
        raw_response.extend_from_slice(&chunk);
        for event in decoder.push(&chunk) {
            let payload: Value = serde_json::from_str(&event.data)
                .context("web search probe returned invalid SSE JSON")?;
            observation.observe(&payload);
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
    if let Some(error) = observation.error {
        anyhow::bail!("web search probe failed: {error}");
    }

    // Run the same mapper used for real requests. The shallow type check alone
    // is not enough: an upstream that omits `server_tool_use.id` or `tool_use_id`
    // must be rejected here instead of being cached as supported.
    let upstream =
        futures_util::stream::iter([Ok::<_, reqwest::Error>(bytes::Bytes::from(raw_response))]);
    let mapped = crate::protocol::openai_events::map_anthropic_sse(
        upstream,
        json!({"model": upstream_model}),
        crate::protocol::convert::ToolNameMap::default(),
    );
    tokio::pin!(mapped);
    let mut mapped_decoder = SseDecoder::default();
    let mut mapped_completed = false;
    let mut mapped_failed = false;
    while let Some(chunk) = mapped.next().await {
        for event in mapped_decoder.push(&chunk.expect("infallible mapper")) {
            match event.event.as_deref() {
                Some("response.completed") => mapped_completed = true,
                Some("response.failed") => mapped_failed = true,
                _ => {}
            }
        }
    }
    if mapped_failed {
        anyhow::bail!("web search probe failed inside the response mapper");
    }
    if observation.ordinary_tool_call {
        return Ok(ProbeVerdict::Unsupported("ordinary_client_tool_call"));
    }
    if observation.server_search_result && observation.message_stop && mapped_completed {
        return Ok(ProbeVerdict::Supported("complete_server_tool_lifecycle"));
    }
    if observation.server_tool_started && !observation.server_search_result {
        anyhow::bail!("web search server tool started without returning a result");
    }
    if !upstream_model.to_ascii_lowercase().starts_with("gpt-") {
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
    pub(super) server_tool_id: Option<String>,
    pub(super) message_stop: bool,
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
            Some("message_stop") => self.message_stop = true,
            _ => {}
        }
    }

    fn observe_content_block(&mut self, block: &Value) {
        match block.get("type").and_then(Value::as_str) {
            Some("server_tool_use")
                if block.get("name").and_then(Value::as_str) == Some("web_search") =>
            {
                match block.get("id").and_then(Value::as_str) {
                    Some(id) if !id.is_empty() => {
                        self.server_tool_started = true;
                        self.server_tool_id = Some(id.to_owned());
                    }
                    _ => self.error = Some("server tool use missing id".to_owned()),
                }
            }
            Some("web_search_tool_result") => {
                match block.get("tool_use_id").and_then(Value::as_str) {
                    Some(id) if self.server_tool_id.as_deref() == Some(id) => {
                        self.server_search_result = true;
                    }
                    Some(_) => {
                        self.error =
                            Some("server tool result tool_use_id does not match".to_owned());
                    }
                    _ => self.error = Some("server tool result missing tool_use_id".to_owned()),
                }
            }
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
