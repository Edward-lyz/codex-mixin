use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use axum::Router;
use codex_mixin::config::GatewayConfig;
use codex_mixin::gateway_access::CODEX_CLIENT_KEY_HEADER;
use codex_mixin::provider::{ProviderModelSource, catalog_model_slug};
use codex_mixin::server::{AppState, router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, header};

async fn spawn_gateway(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{address}")
}

#[tokio::test]
#[ignore = "requires local provider credentials and makes real model requests"]
async fn all_registered_models_can_say_hi() {
    let config = GatewayConfig::from_stored_config().expect("load local Codex Mixin configuration");
    let gateway_api_key = config.gateway_api_key.clone();
    let state = AppState::new(config).unwrap();
    let custom_models = state
        .fetch_models()
        .await
        .expect("fetch custom upstream models")
        .into_iter()
        .map(|model| model.id)
        .collect::<HashSet<_>>();
    let gateway_url = spawn_gateway(router(state)).await;
    let catalog_path = std::env::var("CODEX_MIXIN_REAL_CATALOG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let codex_home = std::env::var("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap()).join(".codex"));
            codex_home.join("model-catalogs/mixin-models.json")
        });
    let catalog: Value = serde_json::from_slice(
        &fs::read(&catalog_path)
            .unwrap_or_else(|err| panic!("read catalog {}: {err}", catalog_path.display())),
    )
    .unwrap_or_else(|err| panic!("parse catalog {}: {err}", catalog_path.display()));
    let managed_upstream_models = catalog["models"]
        .as_array()
        .expect("catalog models must be an array")
        .iter()
        .filter_map(|model| model["codex_mixin_upstream_model"].as_str())
        .collect::<HashSet<_>>();
    let mut models = catalog["models"]
        .as_array()
        .expect("catalog models must be an array")
        .iter()
        .filter(|model| {
            model["codex_mixin_upstream_model"]
                .as_str()
                .is_some_and(|upstream| custom_models.contains(upstream))
                || model["slug"].as_str().is_some_and(|slug| {
                    custom_models.contains(slug) && !managed_upstream_models.contains(slug)
                })
        })
        .map(|model| {
            model["slug"]
                .as_str()
                .expect("registered model must have a slug")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        models.len(),
        custom_models.len(),
        "not every custom upstream model is registered in the catalog"
    );
    if let Ok(model) = std::env::var("CODEX_MIXIN_REAL_MODEL") {
        models.retain(|candidate| candidate == &model);
        assert_eq!(
            models.len(),
            1,
            "CODEX_MIXIN_REAL_MODEL is not registered: {model}"
        );
    }

    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut failures = Vec::new();
    for (index, model) in models.iter().enumerate() {
        let started = Instant::now();
        let body = json!({
            "type": "response.create",
            "model": model,
            "store": false,
            "max_output_tokens": 2048,
            "instructions": "Reply with hi only.",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Say hi."}]
            }]
        });
        let mut request = format!("{websocket_url}/v1/responses")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "session-id",
            HeaderValue::from_str(&format!("codex-mixin-smoke-{index}")).unwrap(),
        );
        if let Some(api_key) = &gateway_api_key {
            request.headers_mut().insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {api_key}")).unwrap(),
            );
        }
        let outcome = match connect_async(request).await {
            Ok((mut socket, _)) => {
                if let Err(err) = socket.send(WsMessage::Text(body.to_string().into())).await {
                    Err(format!("send websocket request: {err}"))
                } else {
                    tokio::time::timeout(Duration::from_secs(180), async {
                        let mut response = String::new();
                        while let Some(message) = socket.next().await {
                            let message = message.map_err(|err| err.to_string())?;
                            if let WsMessage::Text(text) = message {
                                response.push_str(&text);
                                response.push('\n');
                                if text.contains("\"type\":\"response.completed\"") {
                                    return Ok(response);
                                }
                                if text.contains("\"type\":\"response.failed\"") {
                                    return Err(response);
                                }
                            }
                        }
                        Err("websocket closed before a terminal response".to_owned())
                    })
                    .await
                    .map_err(|_| "timed out after 180 seconds".to_owned())
                    .and_then(|result| result)
                }
            }
            Err(err) => Err(format!("connect websocket: {err}")),
        };
        match outcome {
            Ok(response)
                if response.contains("response.output_text.delta")
                    && response.contains("response.completed") =>
            {
                println!("PASS {model} ({} ms)", started.elapsed().as_millis());
            }
            Ok(response) | Err(response) => {
                let reversed_excerpt = response.chars().rev().take(1000).collect::<String>();
                let excerpt = reversed_excerpt.chars().rev().collect::<String>();
                failures.push(format!("{model}: {excerpt}"));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} registered models failed:\n{}",
        failures.len(),
        models.len(),
        failures.join("\n")
    );
}

#[tokio::test]
#[ignore = "requires local Baidu OneAPI credentials and makes real model requests"]
async fn baidu_reasoning_profiles_reach_real_upstream() {
    let mut config =
        GatewayConfig::from_stored_config().expect("load local Codex Mixin configuration");
    let gateway_api_key = config.gateway_api_key.clone();
    let provider_index = config
        .providers
        .iter()
        .position(|provider| provider.model_source == ProviderModelSource::BaiduOneApi)
        .expect("Baidu OneAPI provider is not configured");
    let mut provider = config.providers.remove(provider_index);
    let provider_id = provider.id.clone();
    let models = [
        "gpt-5.6-sol",
        "Claude Opus 4.6",
        "DeepSeek-V4-Flash",
        "GLM-5.2",
    ];
    for model in models {
        assert!(
            provider
                .cached_models
                .iter()
                .any(|candidate| candidate.id == model),
            "Baidu OneAPI catalog does not contain {model}"
        );
        if !provider
            .selected_models
            .iter()
            .any(|candidate| candidate == model)
        {
            provider.selected_models.push(model.to_owned());
        }
    }
    config.providers = vec![provider];
    config.fusion_profiles.clear();
    let gateway_url = spawn_gateway(router(AppState::new(config).unwrap())).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .unwrap();

    let mut catalog_request = client.get(format!("{gateway_url}/v1/codex-model-catalog"));
    if let Some(api_key) = &gateway_api_key {
        catalog_request = catalog_request.bearer_auth(api_key);
    }
    let catalog: Value = catalog_request
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    for (model, expected_efforts) in [
        (
            "gpt-5.6-sol",
            &["none", "low", "medium", "high", "xhigh", "max", "ultra"][..],
        ),
        (
            "Claude Opus 4.6",
            &["none", "low", "medium", "high", "xhigh", "max", "ultra"][..],
        ),
        (
            "DeepSeek-V4-Flash",
            &["none", "low", "medium", "high", "xhigh", "max", "ultra"][..],
        ),
        (
            "GLM-5.2",
            &["none", "low", "medium", "high", "xhigh", "max", "ultra"][..],
        ),
    ] {
        let slug = catalog_model_slug(model, &provider_id);
        let profile = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["slug"] == slug)
            .unwrap_or_else(|| panic!("catalog does not contain {slug}"));
        let efforts = profile["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|level| level["effort"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(efforts, expected_efforts, "{model}");
        assert_eq!(profile["multi_agent_version"], "v2", "{model}");
    }

    for (index, model) in models.into_iter().enumerate() {
        for effort in ["xhigh", "max", "ultra"] {
            let started = Instant::now();
            let mut request = client
                .post(format!("{gateway_url}/v1/responses"))
                .header("session-id", format!("real-{effort}-{index}"))
                .json(&json!({
                    "model": catalog_model_slug(model, &provider_id),
                    "stream": true,
                    "store": false,
                    "max_output_tokens": 1024,
                    "reasoning": {"effort": effort},
                    "instructions": "Reply with hi only.",
                    "input": [{
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "Say hi."}]
                    }],
                    "tools": []
                }));
            if let Some(api_key) = &gateway_api_key {
                request = request.bearer_auth(api_key);
            }
            let response = request.send().await.unwrap();
            let status = response.status();
            let body = response.text().await.unwrap();
            assert!(
                status.is_success(),
                "{model} {effort} returned {status}: {body}"
            );
            assert!(
                body.contains("response.completed"),
                "{model} {effort} did not complete: {body}"
            );
            assert!(
                !body.contains("response.failed"),
                "{model} {effort} failed: {body}"
            );
            println!(
                "PASS {model} {effort} ({} ms)",
                started.elapsed().as_millis()
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires local Baidu OneAPI credentials and makes a real model request"]
async fn baidu_auxiliary_auto_review_alias_reaches_real_upstream() {
    let mut config =
        GatewayConfig::from_stored_config().expect("load local Codex Mixin configuration");
    let gateway_api_key = config.gateway_api_key.clone();
    let codex_client_key = config.gateway_client_keys.codex.clone();
    let provider_index = config
        .providers
        .iter()
        .position(|provider| provider.model_source == ProviderModelSource::BaiduOneApi)
        .expect("Baidu OneAPI provider is not configured");
    let provider = config.providers.remove(provider_index);
    assert!(provider.auxiliary_model_upstream);
    config.providers = vec![provider];
    config.fusion_profiles.clear();
    config.accept_codex_oauth = false;

    let gateway_url = spawn_gateway(router(AppState::new(config).unwrap())).await;
    let mut request = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .header("session-id", "codex-mixin-real-auto-review")
        .json(&json!({
            "model": "codex-auto-review",
            "store": false,
            "stream": true,
            "max_output_tokens": 1,
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Reply OK only."}]
            }]
        }));
    if let Some(api_key) = gateway_api_key {
        request = request.bearer_auth(api_key);
    }
    if let Some(client_key) = codex_client_key {
        request = request.header(CODEX_CLIENT_KEY_HEADER, client_key);
    }

    let response = request
        .send()
        .await
        .expect("send real auxiliary auto-review request");
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    assert!(
        status.is_success(),
        "Baidu auxiliary auto-review request failed with {status}: {}",
        body.chars().take(1000).collect::<String>()
    );
}
