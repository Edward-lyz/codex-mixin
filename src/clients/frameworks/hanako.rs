use std::path::{Path, PathBuf};

use anyhow::Context;
use reqwest::Method;
use serde_json::{Value, json};

use super::{FrameworkModel, engine, native_http};
use crate::clients::IntegrationStatus;

const DETAIL: &str = "Hanako must be running. Provider changes use its authenticated local API; its encrypted catalog is never edited.";

fn connection(root: &Path) -> anyhow::Result<(String, String, String)> {
    let info = engine::read_config(&root.join("server-info.json"), engine::Format::Json)?;
    let port = info["port"]
        .as_u64()
        .filter(|port| *port > 0 && *port <= u64::from(u16::MAX))
        .context("open Hanako before connecting; its native server port is unavailable")?;
    let token = info["token"]
        .as_str()
        .filter(|token| !token.is_empty() && !token.chars().any(char::is_control))
        .context("Hanako native API token is unavailable")?
        .to_owned();
    let endpoint = format!("http://127.0.0.1:{port}");
    let identity = native_http::request(
        Method::GET,
        &format!("{endpoint}/api/server/identity"),
        Some(&token),
        None,
    )?;
    let identity = identity["serverId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .context("Hanako native server identity was not confirmed")?
        .to_owned();
    Ok((endpoint, token, identity))
}

fn agent(root: &Path) -> anyhow::Result<String> {
    let preferences =
        engine::read_config(&root.join("user/preferences.json"), engine::Format::Json)?;
    if let Some(primary) = preferences["primaryAgent"].as_str() {
        native_http::identifier(primary)?;
        if root
            .join("agents")
            .join(primary)
            .join("config.yaml")
            .is_file()
        {
            return Ok(primary.to_owned());
        }
    }
    let mut agents = std::fs::read_dir(root.join("agents"))
        .context("Hanako has no configured native agent")?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    agents.sort();
    for path in agents {
        if !path.join("config.yaml").is_file() {
            continue;
        }
        let id = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("invalid Hanako agent directory")?;
        native_http::identifier(id)?;
        return Ok(id.to_owned());
    }
    anyhow::bail!("open Hanako and create a native agent before connecting")
}

fn chat(root: &Path, agent: &str) -> anyhow::Result<Value> {
    native_http::identifier(agent)?;
    let config = engine::read_config(
        &root.join("agents").join(agent).join("config.yaml"),
        engine::Format::Yaml,
    )?;
    Ok(config
        .pointer("/models/chat")
        .cloned()
        .unwrap_or(Value::Null))
}

fn reports(endpoint: &str, token: &str) -> anyhow::Result<Vec<Value>> {
    native_http::request(
        Method::GET,
        &format!("{endpoint}/api/models"),
        Some(token),
        None,
    )?["models"]
        .as_array()
        .cloned()
        .context("invalid Hanako native model report")
}

fn request(endpoint: &str, token: &str, path: &str, body: Value) -> anyhow::Result<()> {
    native_http::request(
        Method::PUT,
        &format!("{endpoint}{path}"),
        Some(token),
        Some(body),
    )?;
    Ok(())
}

pub(super) fn install(
    manifest: &Path,
    base: &str,
    key: &str,
    models: &[FrameworkModel],
) -> anyhow::Result<()> {
    let root = super::home_path("HANA_HOME", ".hanako")?;
    install_at(manifest, &root, (base, key), models)
}

fn install_at(
    manifest: &Path,
    root: &Path,
    binding: (&str, &str),
    models: &[FrameworkModel],
) -> anyhow::Result<()> {
    let first = models
        .first()
        .context("no selected models are available for Hanako")?;
    let (endpoint, token, identity) = connection(root)?;
    let mut record = match engine::read_record(manifest)? {
        Some(record) => {
            anyhow::ensure!(
                record["root"] == json!(root)
                    && record["identity"] == identity
                    && record["base"] == format!("{}/v1", binding.0),
                "Hanako installation changed; restore its previous integration first"
            );
            if record["phase"] == "managed" {
                anyhow::ensure!(
                    status(manifest)?.state == "managed",
                    "Hanako integration drifted"
                );
                return Ok(());
            }
            anyhow::ensure!(
                record["disconnecting"] != true,
                "finish removing the previous Hanako provider first"
            );
            record
        }
        None => {
            let agent = agent(root)?;
            let id = format!("codex-mixin-{}", uuid::Uuid::new_v4().simple());
            json!({"kind":"hanako","phase":"preparing","root":root,"identity":identity,
                "provider":id,"agent":agent,"before":chat(root,&agent)?,
                "after":{"provider":id,"id":first.id},"base":format!("{}/v1",binding.0),
                "models":models.iter().map(|model|&model.id).collect::<Vec<_>>(),"disconnecting":false})
        }
    };
    let id = native_http::identifier(
        record["provider"]
            .as_str()
            .context("invalid Hanako provider record")?,
    )?
    .to_owned();
    let agent = native_http::identifier(
        record["agent"]
            .as_str()
            .context("invalid Hanako agent record")?,
    )?
    .to_owned();
    engine::write_record(manifest, &record)?;
    let definitions: Vec<_>=models.iter().map(|model|json!({"id":model.id,"name":model.name,
        "context":model.context_window,"maxOutput":model.max_tokens,"image":model.supports_image,
        "reasoning":!model.efforts.is_empty(),"xhigh":model.efforts.iter().any(|effort|effort=="xhigh")})).collect();
    request(
        &endpoint,
        &token,
        "/api/config",
        json!({"providers":{&id:{"display_name":"Codex Mixin",
        "base_url":record["base"],"api":"openai-completions","api_key":binding.1,"models":definitions}}}),
    )?;
    let current = chat(root, &agent)?;
    anyhow::ensure!(
        current == record["before"] || current == record["after"],
        "Hanako chat selection changed during install"
    );
    request(
        &endpoint,
        &token,
        &format!("/api/agents/{agent}/config"),
        json!({"models":{"chat":record["after"]}}),
    )?;
    record["phase"] = json!("managed");
    engine::write_record(manifest, &record)?;
    anyhow::ensure!(
        status(manifest)?.state == "managed",
        "Hanako did not confirm its provider and selected model"
    );
    Ok(())
}

pub(super) fn status(manifest: &Path) -> anyhow::Result<IntegrationStatus> {
    let Some(record) = engine::read_record(manifest)? else {
        return Ok(IntegrationStatus {
            state: "unmanaged",
            mode: Some("native_app"),
            detail: DETAIL,
            error: None,
        });
    };
    let root = PathBuf::from(
        record["root"]
            .as_str()
            .context("invalid Hanako integration record")?,
    );
    let (endpoint, token, identity) = connection(&root)?;
    anyhow::ensure!(
        record["identity"] == identity,
        "Hanako native application identity changed"
    );
    let models = reports(&endpoint, &token)?;
    let expected: std::collections::BTreeSet<_> = record["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let actual: std::collections::BTreeSet<_> = models
        .iter()
        .filter(|model| model["provider"] == record["provider"])
        .filter_map(|model| model["id"].as_str())
        .collect();
    let selected = chat(
        &root,
        record["agent"]
            .as_str()
            .context("invalid Hanako agent record")?,
    )?;
    let confirmed = expected == actual && selected == record["after"];
    Ok(IntegrationStatus {
        state: if record["disconnecting"] == true {
            "pending_removal"
        } else if confirmed {
            "managed"
        } else if record["phase"] == "managed" {
            "drift"
        } else {
            "pending"
        },
        mode: Some("native_app"),
        detail: if record["disconnecting"] == true {
            "Gateway access is revoked. Select another native chat model in Hanako, then disconnect again to remove the owned provider."
        } else {
            DETAIL
        },
        error: (!confirmed && record["disconnecting"] != true)
            .then(|| "Hanako has not confirmed its model selection and provider catalog.".into()),
    })
}

pub(super) fn remove(manifest: &Path) -> anyhow::Result<()> {
    let Some(mut record) = engine::read_record(manifest)? else {
        return Ok(());
    };
    let root = PathBuf::from(
        record["root"]
            .as_str()
            .context("invalid Hanako integration record")?,
    );
    let (endpoint, token, identity) = connection(&root)?;
    anyhow::ensure!(
        record["identity"] == identity,
        "Hanako native application identity changed"
    );
    let id = native_http::identifier(
        record["provider"]
            .as_str()
            .context("invalid Hanako provider record")?,
    )?
    .to_owned();
    let agent = native_http::identifier(
        record["agent"]
            .as_str()
            .context("invalid Hanako agent record")?,
    )?
    .to_owned();
    let current = chat(&root, &agent)?;
    let models = reports(&endpoint, &token)?;
    let previous = models
        .iter()
        .find(|model| {
            model["provider"] == record["before"]["provider"]
                && model["id"] == record["before"]["id"]
        })
        .or_else(|| {
            models.iter().find(|model| {
                model["provider"] != id && model["provider"].is_string() && model["id"].is_string()
            })
        });
    if current == record["after"] {
        let Some(previous) = previous else {
            record["disconnecting"] = json!(true);
            return engine::write_record(manifest, &record);
        };
        let restored = json!({"provider":previous["provider"],"id":previous["id"]});
        record["restore"] = restored.clone();
        record["phase"] = json!("removing");
        engine::write_record(manifest, &record)?;
        request(
            &endpoint,
            &token,
            &format!("/api/agents/{agent}/config"),
            json!({"models":{"chat":restored}}),
        )?;
    } else {
        anyhow::ensure!(
            current == record["before"]
                || current == record["restore"]
                || record["disconnecting"] == true,
            "Hanako chat selection was edited; reconcile it before disconnecting"
        );
        anyhow::ensure!(
            current["provider"] != id,
            "select another native Hanako model before removing the provider"
        );
    }
    record["phase"] = json!("removing");
    engine::write_record(manifest, &record)?;
    if models.iter().any(|model| model["provider"] == id) {
        request(
            &endpoint,
            &token,
            "/api/config",
            json!({"providers":{id:Value::Null}}),
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
        http::{HeaderMap, Method, StatusCode, Uri},
        response::{IntoResponse, Response},
        routing::any,
    };
    use std::sync::{Arc, Mutex};

    struct Fixture {
        directory: tempfile::TempDir,
        state: Arc<Mutex<Value>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("agents/primary")).unwrap();
        std::fs::create_dir_all(root.join("user")).unwrap();
        std::fs::write(
            root.join("user/preferences.json"),
            r#"{"primaryAgent":"primary"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("agents/primary/config.yaml"),
            "models:\n  chat:\n    id: old\n    provider: native\ntheme: custom\n",
        )
        .unwrap();
        std::fs::write(
            root.join("provider-catalog.json"),
            "HANA-SECRET-1.AES-256-GCM.encrypted-fixture",
        )
        .unwrap();
        let state = Arc::new(Mutex::new(
            json!({"root":root,"providers":{},"fail_delete":false,
            "native_models":[{"id":"old","provider":"native"}]}),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(
            root.join("server-info.json"),
            json!({"port":port,"token":"fixture-token"}).to_string(),
        )
        .unwrap();
        let router = Router::new()
            .fallback(any(handle))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Fixture {
            directory,
            state,
            task,
        }
    }

    async fn handle(
        State(state): State<Arc<Mutex<Value>>>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        assert_eq!(
            headers.get("authorization").unwrap(),
            "Bearer fixture-token"
        );
        let mut state = state.lock().unwrap();
        let input: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        let value = match (method, uri.path()) {
            (Method::GET, "/api/server/identity") => json!({"serverId":"native-fixture"}),
            (Method::GET, "/api/models") => {
                let mut models = state["native_models"].as_array().unwrap().clone();
                for (provider, entry) in state["providers"].as_object().unwrap() {
                    for model in entry["models"].as_array().unwrap() {
                        models.push(json!({"id":model["id"],"provider":provider}));
                    }
                }
                json!({"models":models})
            }
            (Method::PUT, "/api/config") => {
                for (id, entry) in input["providers"].as_object().unwrap() {
                    if entry.is_null() {
                        if state["fail_delete"] == true {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        state["providers"].as_object_mut().unwrap().remove(id);
                    } else {
                        assert!(id.starts_with("codex-mixin-"));
                        assert_eq!(entry["api"], "openai-completions");
                        assert_eq!(entry["api_key"], "fixture-key");
                        assert_eq!(entry["base_url"], "http://127.0.0.1:8080/v1");
                        assert_eq!(entry["models"][0]["context"], 64_000);
                        state["providers"][id] = entry.clone();
                    }
                }
                Value::Null
            }
            (Method::PUT, "/api/agents/primary/config") => {
                assert!(input["models"]["chat"]["provider"].is_string());
                let path = PathBuf::from(state["root"].as_str().unwrap())
                    .join("agents/primary/config.yaml");
                let mut original: Value =
                    serde_yaml::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                original["models"]["chat"] = input["models"]["chat"].clone();
                std::fs::write(path, serde_yaml::to_string(&original).unwrap()).unwrap();
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
    async fn sealed_catalog_is_not_touched_and_native_failures_can_resume_removal() {
        let fixture = fixture().await;
        let root = fixture.directory.path().canonicalize().unwrap();
        let state = fixture.state.clone();
        tokio::task::spawn_blocking(move || {
            let manifest = root.join("integration.json");
            let sealed = std::fs::read(root.join("provider-catalog.json")).unwrap();
            install_at(
                &manifest,
                &root,
                ("http://127.0.0.1:8080", "fixture-key"),
                &models(),
            )
            .unwrap();
            assert_eq!(status(&manifest).unwrap().state, "managed");
            install_at(
                &manifest,
                &root,
                ("http://127.0.0.1:8080", "fixture-key"),
                &models(),
            )
            .unwrap();
            assert_eq!(
                state.lock().unwrap()["providers"]
                    .as_object()
                    .unwrap()
                    .len(),
                1
            );
            state.lock().unwrap()["fail_delete"] = json!(true);
            assert!(remove(&manifest).is_err());
            assert!(manifest.exists());
            assert_eq!(
                chat(&root, "primary").unwrap(),
                json!({"id":"old","provider":"native"})
            );
            state.lock().unwrap()["fail_delete"] = json!(false);
            remove(&manifest).unwrap();
            assert_eq!(state.lock().unwrap()["providers"], json!({}));
            assert_eq!(
                std::fs::read(root.join("provider-catalog.json")).unwrap(),
                sealed
            );
            let settings = engine::read_config(
                &root.join("agents/primary/config.yaml"),
                engine::Format::Yaml,
            )
            .unwrap();
            assert_eq!(settings["theme"], "custom");
            assert_eq!(
                settings["models"]["chat"],
                json!({"id":"old","provider":"native"})
            );
            assert!(!manifest.exists());
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_restore_model_requires_native_user_action_before_provider_removal() {
        let fixture = fixture().await;
        let root = fixture.directory.path().canonicalize().unwrap();
        let state = fixture.state.clone();
        tokio::task::spawn_blocking(move || {
            let manifest = root.join("integration.json");
            install_at(
                &manifest,
                &root,
                ("http://127.0.0.1:8080", "fixture-key"),
                &models(),
            )
            .unwrap();
            state.lock().unwrap()["native_models"] = json!([]);
            remove(&manifest).unwrap();
            assert_eq!(status(&manifest).unwrap().state, "pending_removal");
            assert_eq!(
                state.lock().unwrap()["providers"]
                    .as_object()
                    .unwrap()
                    .len(),
                1
            );
            let path = root.join("agents/primary/config.yaml");
            let mut settings = engine::read_config(&path, engine::Format::Yaml).unwrap();
            settings["models"]["chat"] = json!({"provider":"new-native","id":"replacement"});
            std::fs::write(path, serde_yaml::to_string(&settings).unwrap()).unwrap();
            state.lock().unwrap()["native_models"] =
                json!([{"provider":"new-native","id":"replacement"}]);
            remove(&manifest).unwrap();
            assert!(!manifest.exists());
        })
        .await
        .unwrap();
    }
}
