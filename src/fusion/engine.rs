use super::analysis::*;
use super::profile::FusionProfile;
use super::prompts::*;
use super::render::*;
use super::routing::{FusionModelProvider, resolve_fusion_model};
use super::types::*;
use super::*;

#[derive(Clone)]
pub struct FusionEngine {
    state: AppState,
    profile: FusionProfile,
    headers: HeaderMap,
}

impl FusionEngine {
    pub fn new(state: &AppState, profile: &FusionProfile) -> Self {
        Self {
            state: state.clone(),
            profile: profile.clone(),
            headers: HeaderMap::new(),
        }
    }

    pub(crate) fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    pub fn stream(self, body: Value) -> ResponseStream {
        self.stream_with_routing(body, None)
    }

    pub(crate) async fn stream_final_continuation(
        &self,
        body: Value,
        routing: Option<&UpstreamRouting>,
    ) -> Result<ResponseStream, GatewayError> {
        let fusion_model = self.profile.model_slug();
        stream_fusion_response(
            &self.state,
            &self.profile.final_model,
            body,
            &self.headers,
            routing,
            Some(&fusion_model),
        )
        .await
    }

    pub(crate) fn stream_with_routing(
        self,
        body: Value,
        routing: Option<UpstreamRouting>,
    ) -> ResponseStream {
        let stream = async_stream::stream! {
            let fusion_model = self.profile.model_slug();
            let task = extract_user_task(&body);
            let cwd = extract_environment_cwd(&task);
            let executor = if self.profile.panel_tools.enabled {
                cwd.as_deref()
                    .and_then(|cwd| match PanelToolExecutor::new(cwd) {
                        Ok(executor) => Some(executor),
                        Err(error) => {
                            tracing::warn!(error = %error, "fusion panel tools disabled: invalid cwd");
                            None
                        }
                    })
            } else {
                None
            };

            let mut pending = FuturesUnordered::new();
            for (index, model) in self.profile.panel_models.iter().cloned().enumerate() {
                let state = self.state.clone();
                let profile = self.profile.clone();
                let task = task.clone();
                let executor = executor.clone();
                let routing = routing.clone();
                let headers = self.headers.clone();
                pending.push(async move {
                    let timeout = Duration::from_millis(profile.timeout_ms);
                    let result = tokio::time::timeout(
                        timeout,
                        run_panel_model(
                            &state,
                            &profile,
                            &model,
                            &task,
                            executor,
                            &headers,
                            routing.as_ref(),
                        ),
                    )
                    .map_err(|_| format!("panel {model} timed out after {} ms", profile.timeout_ms))
                    .and_then(|result| async move { result })
                    .await;
                    (index, model, result)
                });
            }

            let total = self.profile.panel_models.len();
            let mut finished = 0;
            let mut successful = Vec::new();
            let mut panel_details = Vec::new();
            while let Some((index, model, result)) = pending.next().await {
                finished += 1;
                match result {
                    Ok(text) => {
                        panel_details.push(FusionPanelDetail {
                            index,
                            model: model.clone(),
                            status: FusionPanelStatus::Completed,
                            text: text.clone(),
                        });
                        successful.push(PanelResult { index, model: model.clone(), text });
                        yield Ok::<Bytes, Infallible>(progress_event(
                            &fusion_model,
                            &format!("panel {model} 完成 ({finished}/{total})…"),
                        ));
                    }
                    Err(error) => {
                        tracing::warn!(model, error = %error, "fusion panel failed");
                        panel_details.push(FusionPanelDetail {
                            index,
                            model: model.clone(),
                            status: FusionPanelStatus::Failed,
                            text: error.clone(),
                        });
                        yield Ok(progress_event(
                            &fusion_model,
                            &format!("panel {model} 失败 ({finished}/{total})…"),
                        ));
                    }
                }
            }
            successful.sort_by_key(|result| result.index);
            panel_details.sort_by_key(|result| result.index);
            let mut details = if self.profile.show_intermediate_results {
                vec![panel_results_detail(&panel_details)]
            } else {
                Vec::new()
            };

            let mut final_body = body.clone();
            final_body["model"] = Value::String(self.profile.final_model.clone());
            final_body["stream"] = Value::Bool(true);
            if successful.len() >= self.profile.min_successful {
                yield Ok(progress_event(&fusion_model, "judge 分析中…"));
                let panel_bundle = format_panel_bundle(&successful);
                let judge_body = judge_request(&self.profile, &panel_bundle);
                let judge_result = tokio::time::timeout(
                    Duration::from_millis(self.profile.timeout_ms),
                    collect_fusion_response(
                        &self.state,
                        &self.profile.judge_model,
                        judge_body,
                        &self.headers,
                        routing.as_ref(),
                    ),
                )
                .await;
                let analysis = match judge_result {
                    Ok(Ok(collected)) => {
                        let analysis = normalize_judge_analysis(&collected.output_text);
                        if self.profile.show_intermediate_results {
                            details.push(judge_result_detail(
                                &self.profile.judge_model,
                                "Completed",
                                analysis.clone(),
                            ));
                        }
                        analysis
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(error = %error, "fusion judge failed; using panel outputs");
                        if self.profile.show_intermediate_results {
                            details.push(judge_result_detail(
                                &self.profile.judge_model,
                                "Failed",
                                format!(
                                    "{error}\n\nFalling back to the successful panel outputs."
                                ),
                            ));
                        }
                        panel_bundle.clone()
                    }
                    Err(_) => {
                        tracing::warn!("fusion judge timed out; using panel outputs");
                        if self.profile.show_intermediate_results {
                            details.push(judge_result_detail(
                                &self.profile.judge_model,
                                "Timed Out",
                                format!(
                                    "Timed out after {} ms. Falling back to the successful panel outputs.",
                                    self.profile.timeout_ms
                                ),
                            ));
                        }
                        panel_bundle.clone()
                    }
                };
                inject_fusion_analysis(&mut final_body, &analysis, &successful);
            } else {
                tracing::warn!(
                    successful = successful.len(),
                    required = self.profile.min_successful,
                    "fusion panels below minimum; falling back to final model"
                );
                yield Ok(progress_event(
                    &fusion_model,
                    "panel 成功数不足，回退 final 模型…",
                ));
                if self.profile.show_intermediate_results {
                    details.push(judge_result_detail(
                        &self.profile.judge_model,
                        "Skipped",
                        format!(
                            "Only {} panel(s) succeeded; {} required. The final model ran without judge analysis.",
                            successful.len(),
                            self.profile.min_successful
                        ),
                    ));
                }
            }

            if self.profile.show_intermediate_results {
                details.push(final_answer_detail(&self.profile.final_model));
            }

            match stream_fusion_response(
                &self.state,
                &self.profile.final_model,
                final_body,
                &self.headers,
                routing.as_ref(),
                Some(&fusion_model),
            )
            .await
            {
                Ok(mut final_stream) => {
                    let rendered_details = render_fusion_details(&details);
                    let detail_count = rendered_details.len() as u64;
                    let detail_items = rendered_details
                        .iter()
                        .map(|detail| detail.item.clone())
                        .collect::<Vec<_>>();
                    let mut decoder = SseDecoder::default();
                    let mut details_emitted = false;
                    while let Some(chunk) = final_stream.next().await {
                        let bytes = match chunk {
                            Ok(bytes) => bytes,
                            Err(never) => match never {},
                        };
                        for event in decoder.push(&bytes) {
                            let event_name = event.event.as_deref().unwrap_or("message");
                            let Ok(mut payload) = serde_json::from_str::<Value>(&event.data) else {
                                yield Ok(encode_raw_event(event_name, &event.data));
                                continue;
                            };
                            let is_created = event_name == "response.created";
                            patch_final_event(
                                &mut payload,
                                detail_count,
                                &detail_items,
                                &fusion_model,
                            );
                            yield Ok(encode_event(event_name, &payload)
                                .expect("fusion final event is serializable"));
                            if is_created && !details_emitted {
                                for detail in &rendered_details {
                                    for event in &detail.events {
                                        yield Ok(event.clone());
                                    }
                                }
                                details_emitted = true;
                            }
                        }
                    }
                    if !decoder.remaining().is_empty() {
                        yield Ok(Bytes::copy_from_slice(decoder.remaining()));
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "fusion final model request failed");
                    yield Ok(failed_event(&fusion_model, &error.to_string()));
                }
            }
        };
        stream.boxed()
    }
}

pub(super) async fn run_panel_model(
    state: &AppState,
    profile: &FusionProfile,
    model: &str,
    task: &str,
    executor: Option<PanelToolExecutor>,
    headers: &HeaderMap,
    routing: Option<&UpstreamRouting>,
) -> Result<String, String> {
    let mut body = panel_request(profile, model, task, executor.is_some());
    let mut rounds = 0;
    let mut calls = 0;
    let mut tool_evidence = Vec::new();
    let mut conclusion_attempted = false;
    loop {
        let collected = collect_fusion_response(state, model, body.clone(), headers, routing)
            .await
            .map_err(|error| error.to_string())?;
        let tool_calls = collected
            .output
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .cloned()
            .collect::<Vec<_>>();
        if tool_calls.is_empty() {
            if collected.output_text.trim().is_empty() {
                return Err(format!("panel {model} returned no text"));
            }
            return normalize_panel_analysis(&collected.output_text)
                .map_err(|error| format!("panel {model} {error}"));
        }
        if conclusion_attempted {
            return Err(format!(
                "panel {model} requested tools during its forced conclusion"
            ));
        }

        let Some(executor) = executor.as_ref() else {
            return Err(format!("panel {model} requested unavailable tools"));
        };
        let input = body
            .get_mut("input")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "panel request input is not an array".to_owned())?;
        input.extend(collected.output);
        for call in tool_calls {
            let call_id = call
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("missing-call-id");
            let name = call.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = call
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let output = if calls < profile.panel_tools.max_calls_per_model {
                calls += 1;
                let executor = executor.clone();
                let name = name.to_owned();
                let arguments = arguments.to_owned();
                tokio::task::spawn_blocking(move || executor.execute(&name, &arguments))
                    .await
                    .map_err(|error| format!("panel tool task failed: {error}"))?
                    .unwrap_or_else(|error| format!("tool error: {error}"))
            } else {
                "tool error: per-model call limit reached".to_owned()
            };
            tool_evidence.push(format!(
                "Tool: {name}\nArguments: {arguments}\nOutput:\n{output}"
            ));
            input.push(json!({
                "type":"function_call_output",
                "call_id":call_id,
                "output":output
            }));
        }
        rounds += 1;
        if rounds >= profile.panel_tools.max_rounds
            || calls >= profile.panel_tools.max_calls_per_model
        {
            body = panel_conclusion_request(profile, model, task, &tool_evidence);
            conclusion_attempted = true;
        }
    }
}

pub(super) async fn collect_fusion_response(
    state: &AppState,
    model_reference: &str,
    body: Value,
    headers: &HeaderMap,
    routing: Option<&UpstreamRouting>,
) -> Result<crate::upstream::CollectedResponse, GatewayError> {
    let stream =
        stream_fusion_response(state, model_reference, body, headers, routing, None).await?;
    collect_response_stream(stream).await
}

pub(super) async fn stream_fusion_response(
    state: &AppState,
    model_reference: &str,
    mut body: Value,
    headers: &HeaderMap,
    routing: Option<&UpstreamRouting>,
    downstream_model: Option<&str>,
) -> Result<ResponseStream, GatewayError> {
    let (provider, model) =
        resolve_fusion_model(model_reference, state.config.provider_preset.as_str());
    body["model"] = Value::String(model);
    match provider {
        FusionModelProvider::Official => {
            body["store"] = Value::Bool(false);
            body.as_object_mut()
                .expect("responses request must be an object")
                .remove("max_output_tokens");
            let stream = stream_official_response(state, headers, body).await?;
            Ok(match downstream_model {
                Some(model) => rewrite_response_model(stream, model.to_owned()),
                None => stream,
            })
        }
        FusionModelProvider::Upstream => {
            stream_response_with_options(state, body, routing, downstream_model).await
        }
    }
}

pub(super) fn rewrite_response_model(
    mut stream: ResponseStream,
    downstream_model: String,
) -> ResponseStream {
    let rewritten = async_stream::stream! {
        let mut decoder = SseDecoder::default();
        while let Some(chunk) = stream.next().await {
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(never) => match never {},
            };
            for event in decoder.push(&bytes) {
                let event_name = event.event.as_deref().unwrap_or("message");
                match serde_json::from_str::<Value>(&event.data) {
                    Ok(mut payload) => {
                        patch_final_event(&mut payload, 0, &[], &downstream_model);
                        yield Ok::<Bytes, Infallible>(encode_event(event_name, &payload)
                            .expect("rewritten official event is serializable"));
                    }
                    Err(_) => yield Ok(encode_raw_event(event_name, &event.data)),
                }
            }
        }
        if !decoder.remaining().is_empty() {
            yield Ok(Bytes::copy_from_slice(decoder.remaining()));
        }
    };
    rewritten.boxed()
}
