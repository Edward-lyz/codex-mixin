use super::*;

const OFFICIAL_MODELS_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

impl AppState {
    pub(crate) async fn official_auth(
        &self,
    ) -> anyhow::Result<(axum::http::HeaderValue, axum::http::HeaderValue)> {
        read_codex_official_auth(&self.config.codex_auth_path, &self.official_auth_cache).await
    }

    pub async fn fetch_official_models_catalog(
        &self,
        client_version: &str,
    ) -> anyhow::Result<Value> {
        self.fetch_official_models_catalog_with_timeout(
            client_version,
            OFFICIAL_MODELS_FETCH_TIMEOUT,
        )
        .await
    }

    async fn fetch_official_models_catalog_with_timeout(
        &self,
        client_version: &str,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let url = official_models_url(&self.config.official_responses_url, client_version)?;
        let (authorization, account_id) = self.official_auth().await?;
        let fetch_catalog = async {
            let response = self
                .client
                .get(url)
                .header(header::AUTHORIZATION, authorization)
                .header("chatgpt-account-id", account_id)
                .header(header::ACCEPT, "application/json")
                .send()
                .await?;
            let status = response.status();
            if !status.is_success() {
                anyhow::bail!("official models endpoint returned {status}");
            }
            let catalog: Value = response.json().await?;
            if catalog.get("models").and_then(Value::as_array).is_none() {
                anyhow::bail!("official models endpoint returned no models array");
            }
            Ok(catalog)
        };
        tokio::time::timeout(timeout, fetch_catalog)
            .await
            .map_err(|_| anyhow::anyhow!("official models endpoint timed out"))?
    }
}
pub(crate) async fn read_codex_official_auth(
    auth_path: &std::path::Path,
    cache: &tokio::sync::Mutex<Option<CachedOfficialAuth>>,
) -> anyhow::Result<(axum::http::HeaderValue, axum::http::HeaderValue)> {
    let metadata = tokio::fs::metadata(auth_path).await.map_err(|err| {
        anyhow::anyhow!("read Codex auth metadata {}: {err}", auth_path.display())
    })?;
    let modified_at = metadata.modified().map_err(|err| {
        anyhow::anyhow!(
            "read Codex auth modification time {}: {err}",
            auth_path.display()
        )
    })?;
    let mut cache = cache.lock().await;
    if let Some(cached) = cache
        .as_ref()
        .filter(|cached| cached.modified_at == modified_at && cached.file_len == metadata.len())
    {
        return Ok((cached.authorization.clone(), cached.account_id.clone()));
    }

    let raw = tokio::fs::read_to_string(auth_path)
        .await
        .map_err(|err| anyhow::anyhow!("read Codex auth file {}: {err}", auth_path.display()))?;
    let auth: Value = serde_json::from_str(&raw)
        .map_err(|err| anyhow::anyhow!("parse Codex auth file {}: {err}", auth_path.display()))?;
    let tokens = auth
        .get("tokens")
        .ok_or_else(|| anyhow::anyhow!("Codex auth file does not contain tokens"))?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Codex auth file does not contain access_token"))?;
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .filter(|account_id| !account_id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Codex auth file does not contain account_id"))?;
    let authorization: axum::http::HeaderValue = format!("Bearer {access_token}").parse()?;
    let account_id: axum::http::HeaderValue = account_id.parse()?;
    *cache = Some(CachedOfficialAuth {
        modified_at,
        file_len: metadata.len(),
        authorization: authorization.clone(),
        account_id: account_id.clone(),
    });
    Ok((authorization, account_id))
}

fn official_models_url(
    official_responses_url: &str,
    client_version: &str,
) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(official_responses_url)?;
    let path = url.path().trim_end_matches('/');
    let prefix = path
        .strip_suffix("/responses")
        .ok_or_else(|| anyhow::anyhow!("official responses URL must end with /responses"))?;
    url.set_path(&format!("{prefix}/models"));
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("client_version", client_version);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use axum::Router;
    use axum::body::Body;
    use axum::response::Response;
    use axum::routing::get;
    use futures_util::stream;

    use super::*;
    use crate::config::ThinkingMode;

    #[tokio::test]
    async fn official_models_timeout_covers_response_body() {
        let upstream = Router::new().route(
            "/models",
            get(|| async {
                Response::builder()
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from_stream(stream::pending::<
                        Result<Bytes, Infallible>,
                    >()))
                    .unwrap()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let auth_path = directory.path().join("auth.json");
        tokio::fs::write(
            &auth_path,
            r#"{"tokens":{"access_token":"secret","account_id":"account-one"}}"#,
        )
        .await
        .unwrap();
        let state = AppState::new(GatewayConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            providers: Vec::new(),
            official_responses_url: format!("http://{address}/responses"),
            codex_auth_path: auth_path,
            gateway_api_key: None,
            gateway_client_keys: crate::gateway_access::GatewayClientKeys::default(),
            accept_codex_oauth: true,
            official_selected_models: None,
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_secs(2),
            thinking_mode: ThinkingMode::Off,
            enable_web_search_tool: false,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            fusion_profiles: Vec::new(),
        })
        .unwrap();

        let fetch =
            state.fetch_official_models_catalog_with_timeout("0.148.0", Duration::from_millis(50));
        let error = tokio::time::timeout(Duration::from_millis(250), fetch)
            .await
            .expect("fetch did not enforce its own timeout")
            .unwrap_err();

        assert_eq!(error.to_string(), "official models endpoint timed out");
    }
}
