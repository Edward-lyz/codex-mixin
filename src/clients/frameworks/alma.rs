use std::path::Path;

use anyhow::Context;
use reqwest::Method;
use serde_json::{Value, json};

use super::{FrameworkModel, engine, native_http};
use crate::clients::IntegrationStatus;

const ENDPOINT: &str = "http://127.0.0.1:23001";
const DETAIL: &str = "Alma must be running. Only the owned provider and chat default are changed; disconnect restores the previous default.";

fn request(
    endpoint: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> anyhow::Result<Value> {
    native_http::request(method, &format!("{endpoint}{path}"), None, body)
}

fn providers(endpoint: &str) -> anyhow::Result<Vec<Value>> {
    request(endpoint, Method::GET, "/api/providers", None)?
        .as_array()
        .cloned()
        .context("invalid Alma provider list")
}

fn owned_provider(record: &Value, providers: Vec<Value>) -> anyhow::Result<Option<Value>> {
    let matches: Vec<_> = providers
        .into_iter()
        .filter(|provider| provider["name"] == record["name"])
        .collect();
    anyhow::ensure!(
        matches.len() <= 1,
        "Alma contains conflicting owned provider names"
    );
    if let Some(provider) = matches.first() {
        anyhow::ensure!(
            provider["baseURL"] == record["base"] && provider["type"] == "openai",
            "Alma owned provider was edited; reconcile it before disconnecting"
        );
        if let Some(id) = record["provider_id"].as_str() {
            anyhow::ensure!(provider["id"] == id, "Alma owned provider identity changed");
        }
    }
    Ok(matches.into_iter().next())
}

fn settings(endpoint: &str) -> anyhow::Result<Value> {
    let settings = request(endpoint, Method::GET, "/api/settings", None)?;
    anyhow::ensure!(
        settings.is_object()
            && settings
                .get("chat")
                .is_none_or(|chat| chat.is_null() || chat.is_object()),
        "invalid Alma settings; refusing to overwrite them"
    );
    Ok(settings)
}

fn set_default(
    settings: &mut Value,
    value: Option<&Value>,
    before_chat: Option<&Value>,
) -> anyhow::Result<()> {
    if !settings.get("chat").is_some_and(Value::is_object) {
        settings["chat"] = json!({});
    }
    let chat = settings["chat"]
        .as_object_mut()
        .context("invalid Alma chat settings")?;
    if let Some(value) = value {
        chat.insert("defaultModel".into(), value.clone());
    } else {
        chat.remove("defaultModel");
    }
    if chat.is_empty() && before_chat.is_none_or(Value::is_null) {
        if before_chat.is_some() {
            settings["chat"] = Value::Null;
        } else {
            settings
                .as_object_mut()
                .context("invalid Alma settings")?
                .remove("chat");
        }
    }
    Ok(())
}

pub(super) fn install(
    manifest: &Path,
    base: &str,
    key: &str,
    models: &[FrameworkModel],
) -> anyhow::Result<()> {
    install_at(manifest, ENDPOINT, (base, key), models)
}

fn install_at(
    manifest: &Path,
    endpoint: &str,
    binding: (&str, &str),
    models: &[FrameworkModel],
) -> anyhow::Result<()> {
    let first = models
        .first()
        .context("no selected models are available for Alma")?;
    let mut record = match engine::read_record(manifest)? {
        Some(record) => {
            anyhow::ensure!(
                record["endpoint"] == endpoint && record["base"] == format!("{}/v1", binding.0),
                "Alma binding changed; disconnect before reconnecting"
            );
            if record["phase"] == "managed" {
                anyhow::ensure!(
                    status_at(manifest)?.state == "managed",
                    "Alma integration drifted"
                );
                return Ok(());
            }
            record
        }
        None => {
            let before = settings(endpoint)?;
            json!({"kind":"alma","phase":"preparing","endpoint":endpoint,
                "name":format!("codex-mixin-{}",uuid::Uuid::new_v4().simple()),"provider_id":null,
                "base":format!("{}/v1",binding.0),"before_chat":before.get("chat"),
                "had_chat":before.get("chat").is_some(),"had_default":before.pointer("/chat/defaultModel").is_some(),
                "before_default":before.pointer("/chat/defaultModel"),
                "models":models.iter().map(|model|&model.id).collect::<Vec<_>>()})
        }
    };
    engine::write_record(manifest, &record)?;
    let provider = owned_provider(&record, providers(endpoint)?)?;
    let provider = if let Some(provider) = provider {
        let id = native_http::identifier(
            provider["id"]
                .as_str()
                .context("invalid Alma provider id")?,
        )?;
        request(
            endpoint,
            Method::PUT,
            &format!("/api/providers/{id}"),
            Some(json!({"apiKey":binding.1})),
        )?;
        provider
    } else {
        request(
            endpoint,
            Method::POST,
            "/api/providers",
            Some(json!({"name":record["name"],"type":"openai",
            "apiKey":binding.1,"baseURL":record["base"],"enabled":true})),
        )?;
        owned_provider(&record, providers(endpoint)?)?
            .context("Alma did not confirm provider creation")?
    };
    let id = native_http::identifier(
        provider["id"]
            .as_str()
            .context("invalid Alma provider id")?,
    )?;
    record["provider_id"] = json!(id);
    record["after_default"] = json!(format!("{id}:{}", first.id));
    engine::write_record(manifest, &record)?;
    let available:Vec<_>=models.iter().map(|model|json!({"id":model.id,"name":model.name,"capabilityOverrides":{
        "reasoning":!model.efforts.is_empty(),"reasoningLevels":model.efforts,"vision":model.supports_image,
        "contextWindow":model.context_window,"maxOutputTokens":model.max_tokens}})).collect();
    request(
        endpoint,
        Method::PUT,
        &format!("/api/providers/{id}/models"),
        Some(json!({"models":record["models"],"availableModels":available})),
    )?;
    let mut current = settings(endpoint)?;
    anyhow::ensure!(
        current.pointer("/chat/defaultModel")
            == record
                .get("before_default")
                .filter(|_| record["had_default"] == true)
            || current.pointer("/chat/defaultModel") == record.get("after_default"),
        "Alma chat default changed during install"
    );
    set_default(
        &mut current,
        record.get("after_default"),
        record.get("before_chat"),
    )?;
    request(endpoint, Method::PUT, "/api/settings", Some(current))?;
    record["phase"] = json!("managed");
    engine::write_record(manifest, &record)?;
    anyhow::ensure!(
        status_at(manifest)?.state == "managed",
        "Alma did not confirm the installed model"
    );
    Ok(())
}

pub(super) fn status(manifest: &Path) -> anyhow::Result<IntegrationStatus> {
    status_at(manifest)
}

fn status_at(manifest: &Path) -> anyhow::Result<IntegrationStatus> {
    let Some(record) = engine::read_record(manifest)? else {
        return Ok(IntegrationStatus {
            state: "unmanaged",
            mode: Some("native_app"),
            detail: DETAIL,
            error: None,
        });
    };
    let endpoint = record["endpoint"]
        .as_str()
        .context("invalid Alma integration record")?;
    let provider = owned_provider(&record, providers(endpoint)?)?;
    let selected = settings(endpoint)?.pointer("/chat/defaultModel").cloned();
    let models_match = provider.as_ref().is_some_and(|provider| {
        let actual: std::collections::BTreeSet<_> = provider["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| {
                model
                    .as_str()
                    .or_else(|| model.get("id").and_then(Value::as_str))
            })
            .collect();
        let expected: std::collections::BTreeSet<_> = record["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        actual == expected && provider["enabled"] == true
    });
    let confirmed = models_match && selected.as_ref() == record.get("after_default");
    Ok(IntegrationStatus {
        state: if confirmed {
            "managed"
        } else if record["phase"] == "managed" {
            "drift"
        } else {
            "pending"
        },
        mode: Some("native_app"),
        detail: DETAIL,
        error: (!confirmed)
            .then(|| "Alma has not confirmed the owned provider, models and default.".into()),
    })
}

pub(super) fn remove(manifest: &Path) -> anyhow::Result<()> {
    let Some(mut record) = engine::read_record(manifest)? else {
        return Ok(());
    };
    let endpoint = record["endpoint"]
        .as_str()
        .context("invalid Alma integration record")?
        .to_owned();
    let provider = owned_provider(&record, providers(&endpoint)?)?;
    if let Some(provider) = provider.as_ref().filter(|_| record["phase"] == "managed") {
        let actual: std::collections::BTreeSet<_> = provider["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| {
                model
                    .as_str()
                    .or_else(|| model.get("id").and_then(Value::as_str))
            })
            .collect();
        let expected: std::collections::BTreeSet<_> = record["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        anyhow::ensure!(
            actual == expected,
            "Alma owned model list was edited; refusing to remove it"
        );
    }
    let mut current = settings(&endpoint)?;
    let selected = current.pointer("/chat/defaultModel");
    let before = record
        .get("before_default")
        .filter(|_| record["had_default"] == true);
    if selected == record.get("after_default") {
        set_default(
            &mut current,
            before,
            record
                .get("before_chat")
                .filter(|_| record["had_chat"] == true),
        )?;
        record["phase"] = json!("removing");
        engine::write_record(manifest, &record)?;
        request(&endpoint, Method::PUT, "/api/settings", Some(current))?;
    } else {
        anyhow::ensure!(
            selected == before,
            "Alma chat selection was edited; refusing to overwrite it"
        );
    }
    if let Some(provider) = provider {
        let id = native_http::identifier(
            provider["id"]
                .as_str()
                .context("invalid Alma provider id")?,
        )?;
        request(
            &endpoint,
            Method::DELETE,
            &format!("/api/providers/{id}"),
            None,
        )?;
    }
    engine::remove_record(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::Bytes,
        extract::State,
        http::{Method, StatusCode, Uri},
        response::{IntoResponse, Response},
        routing::any,
    };
    use std::sync::{Arc, Mutex};

    struct Fixture {
        endpoint: String,
        state: Arc<Mutex<Value>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn fixture() -> Fixture {
        let state = Arc::new(Mutex::new(json!({
            "providers":[{"id":"native","name":"Original","type":"openai","baseURL":"http://native.invalid/v1","models":["old"]}],
            "settings":{"chat":{"defaultModel":"native:old","temperature":0.3},"theme":"custom"},
            "fail_models":false
        })));
        let router = Router::new()
            .fallback(any(handle))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Fixture {
            endpoint,
            state,
            task,
        }
    }

    async fn handle(
        State(state): State<Arc<Mutex<Value>>>,
        method: Method,
        uri: Uri,
        body: Bytes,
    ) -> Response {
        let mut state = state.lock().unwrap();
        let path = uri.path();
        let input: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        let value = match (method, path) {
            (Method::GET, "/api/providers") => state["providers"].clone(),
            (Method::GET, "/api/settings") => state["settings"].clone(),
            (Method::PUT, "/api/settings") => {
                state["settings"] = input;
                Value::Null
            }
            (Method::POST, "/api/providers") => {
                assert_eq!(input["type"], "openai");
                assert_eq!(input["baseURL"], "http://127.0.0.1:8080/v1");
                assert_eq!(input["apiKey"], "fixture-key");
                let mut provider = input;
                provider["id"] = json!("owned-id");
                provider["models"] = json!([]);
                state["providers"].as_array_mut().unwrap().push(provider);
                Value::Null
            }
            (Method::PUT, "/api/providers/owned-id/models") => {
                if state["fail_models"] == true {
                    return (StatusCode::SERVICE_UNAVAILABLE, "private upstream detail")
                        .into_response();
                }
                assert_eq!(
                    input["availableModels"][0]["capabilityOverrides"]["contextWindow"],
                    64_000
                );
                let provider = state["providers"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|provider| provider["id"] == "owned-id")
                    .unwrap();
                provider["models"] = input["models"].clone();
                provider["availableModels"] = input["availableModels"].clone();
                Value::Null
            }
            (Method::PUT, "/api/providers/owned-id") => {
                state["providers"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|provider| provider["id"] == "owned-id")
                    .unwrap()["apiKey"] = input["apiKey"].clone();
                Value::Null
            }
            (Method::DELETE, "/api/providers/owned-id") => {
                state["providers"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|provider| provider["id"] != "owned-id");
                Value::Null
            }
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
        Json(value).into_response()
    }

    fn models() -> Vec<FrameworkModel> {
        vec![FrameworkModel {
            id: "provider/model".into(),
            name: "Model".into(),
            context_window: 64_000,
            max_tokens: 4096,
            supports_image: true,
            protocol: crate::provider::ProviderProtocol::OpenAiChat,
            efforts: vec!["high".into()],
        }]
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_api_roundtrip_restores_defaults_and_refuses_managed_drift() {
        let fixture = fixture().await;
        let endpoint = fixture.endpoint.clone();
        let state = fixture.state.clone();
        tokio::task::spawn_blocking(move || {
            let directory=tempfile::tempdir().unwrap();
            let manifest=directory.path().join("alma.json");
            install_at(&manifest,&endpoint,("http://127.0.0.1:8080","fixture-key"),&models()).unwrap();
            assert_eq!(status_at(&manifest).unwrap().state,"managed");
            install_at(&manifest,&endpoint,("http://127.0.0.1:8080","fixture-key"),&models()).unwrap();
            assert_eq!(state.lock().unwrap()["providers"].as_array().unwrap().len(),2);
            state.lock().unwrap()["settings"]["theme"]=json!("user-edited");
            state.lock().unwrap()["settings"]["chat"]["defaultModel"]=json!("native:another");
            assert!(remove(&manifest).is_err());
            assert!(manifest.exists());
            state.lock().unwrap()["settings"]["chat"]["defaultModel"]=json!("owned-id:provider/model");
            remove(&manifest).unwrap();
            let state=state.lock().unwrap();
            assert_eq!(state["providers"].as_array().unwrap().len(),1);
            assert_eq!(state["providers"][0]["id"],"native");
            assert_eq!(state["settings"],json!({"chat":{"defaultModel":"native:old","temperature":0.3},"theme":"user-edited"}));
            assert!(!manifest.exists());
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn partial_native_failure_is_retryable_without_duplicate_providers_or_secret_errors() {
        let fixture = fixture().await;
        fixture.state.lock().unwrap()["fail_models"] = json!(true);
        let endpoint = fixture.endpoint.clone();
        let state = fixture.state.clone();
        tokio::task::spawn_blocking(move || {
            let directory = tempfile::tempdir().unwrap();
            let manifest = directory.path().join("alma.json");
            let error = install_at(
                &manifest,
                &endpoint,
                ("http://127.0.0.1:8080", "fixture-key"),
                &models(),
            )
            .unwrap_err();
            assert!(!format!("{error:#}").contains("private upstream detail"));
            assert!(manifest.exists());
            assert_eq!(
                state.lock().unwrap()["settings"]["chat"]["defaultModel"],
                "native:old"
            );
            state.lock().unwrap()["fail_models"] = json!(false);
            install_at(
                &manifest,
                &endpoint,
                ("http://127.0.0.1:8080", "fixture-key"),
                &models(),
            )
            .unwrap();
            assert_eq!(
                state.lock().unwrap()["providers"].as_array().unwrap().len(),
                2
            );
            remove(&manifest).unwrap();
        })
        .await
        .unwrap();
    }
}
