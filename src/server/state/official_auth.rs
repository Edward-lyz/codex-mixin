use super::*;

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
        let url = official_models_url(&self.config.official_responses_url, client_version)?;
        let (authorization, account_id) = self.official_auth().await?;
        let response = tokio::time::timeout(
            Duration::from_secs(10),
            self.client
                .get(url)
                .header(header::AUTHORIZATION, authorization)
                .header("chatgpt-account-id", account_id)
                .header(header::ACCEPT, "application/json")
                .send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("official models endpoint timed out"))??;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("official models endpoint returned {status}");
        }
        let catalog: Value = response.json().await?;
        if catalog.get("models").and_then(Value::as_array).is_none() {
            anyhow::bail!("official models endpoint returned no models array");
        }
        Ok(catalog)
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
