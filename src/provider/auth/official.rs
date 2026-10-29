//! Official OAuth credentials shared by discovery, quota and inference.
use anyhow::{Context, ensure};
use reqwest::header::HeaderValue;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::Mutex;

const MAX_AUTH_BYTES: u64 = 1024 * 1024;
const MAX_CATALOG_BYTES: u64 = 8 * 1024 * 1024;
const CODEX_COMPAT_VERSION: &str = "0.159.0";
pub(crate) struct CachedOfficialAuth {
    path: PathBuf,
    modified: Option<(SystemTime, u64)>,
    expires: Instant,
    authorization: HeaderValue,
    account: HeaderValue,
}

fn effective_path(path: &Path) -> PathBuf {
    if path == crate::platform::oauth::codex_auth_path() {
        return crate::platform::oauth::official_auth_path();
    }
    path.to_owned()
}

fn read_document(path: &Path) -> anyhow::Result<Option<Value>> {
    use std::io::Read;
    let bytes = match std::fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(MAX_AUTH_BYTES + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_AUTH_BYTES,
                "Codex credentials exceed 1 MiB"
            );
            Some(bytes)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::platform::oauth::codex_keyring(path)?
        }
        Err(error) => return Err(error).context("read official OAuth credentials"),
    };
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let document: Value =
        serde_json::from_slice(&bytes).context("invalid official OAuth credentials JSON")?;
    if document.get("tokens").is_none_or(Value::is_null) {
        return Ok(None);
    }
    Ok(Some(document))
}

pub fn available(path: &Path) -> anyhow::Result<bool> {
    Ok(account_id(path)?.is_some())
}

pub fn account_id(path: &Path) -> anyhow::Result<Option<String>> {
    let path = effective_path(path);
    let Some(document) = read_document(&path)? else {
        return Ok(None);
    };
    let (_, account) = auth_pair(&document)?;
    Ok(Some(account.to_str()?.to_owned()))
}

fn auth_pair(document: &Value) -> anyhow::Result<(HeaderValue, HeaderValue)> {
    let token = document
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .context("official OAuth access_token is missing")?;
    let account = document
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .context("official OAuth account_id is missing")?;
    Ok((format!("Bearer {token}").parse()?, account.parse()?))
}

pub(crate) async fn headers(
    path: &Path,
    cache: &Mutex<Option<CachedOfficialAuth>>,
) -> anyhow::Result<(HeaderValue, HeaderValue)> {
    let path = effective_path(path);
    let modified = match tokio::fs::metadata(&path).await {
        Ok(metadata) => Some((metadata.modified()?, metadata.len())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("read official OAuth metadata"),
    };
    let mut cache = cache.lock().await;
    if let Some(cached) = cache.as_ref().filter(|cached| {
        cached.path == path && cached.modified == modified && cached.expires > Instant::now()
    }) {
        return Ok((cached.authorization.clone(), cached.account.clone()));
    }
    let read_path = path.clone();
    let mut document = tokio::task::spawn_blocking(move || read_document(&read_path))
        .await??
        .context("official OAuth login is missing; run providers add --preset openai")?;
    if needs_refresh(&document) {
        // Imported official sessions are read-only. Rotating their refresh token
        // independently can sign the official app out of its existing session.
        ensure!(
            path == crate::platform::oauth::oauth_path("official"),
            "local Codex OAuth access token expired; refresh the official session or run providers add --preset openai for independent renewal"
        );
        let _refresh_lock = tokio::task::spawn_blocking(|| {
            crate::platform::oauth::lock_oauth(&crate::platform::oauth::oauth_path("official"))
        })
        .await??;
        let latest_path = path.clone();
        document = tokio::task::spawn_blocking(move || read_document(&latest_path))
            .await??
            .context("owned OAuth credentials disappeared during refresh")?;
        if needs_refresh(&document) {
            document = refresh_owned(document).await?;
        }
    }
    let (authorization, account) = auth_pair(&document)?;
    *cache = Some(CachedOfficialAuth {
        path,
        modified,
        expires: Instant::now() + Duration::from_secs(30),
        authorization: authorization.clone(),
        account: account.clone(),
    });
    Ok((authorization, account))
}

fn needs_refresh(document: &Value) -> bool {
    document
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .and_then(super::oauth::jwt_claims)
        .and_then(|claims| claims["exp"].as_i64())
        .is_some_and(|expiry| expiry <= chrono::Utc::now().timestamp() + 60)
}

async fn refresh_owned(mut document: Value) -> anyhow::Result<Value> {
    let refresh = document
        .pointer("/tokens/refresh_token")
        .and_then(Value::as_str)
        .context("expired official OAuth credentials have no refresh_token")?;
    let refreshed = super::oauth::token_request(
        "https://auth.openai.com/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("client_id", super::oauth::OPENAI_CLIENT_ID),
            ("refresh_token", refresh),
        ],
    )
    .await?;
    ensure!(
        refreshed["access_token"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "OpenAI refresh response has no access_token"
    );
    for key in ["access_token", "refresh_token", "id_token"] {
        if refreshed.get(key).is_some() {
            document["tokens"][key] = refreshed[key].clone();
        }
    }
    document["last_refresh"] = json!(chrono::Utc::now().to_rfc3339());
    let owned = crate::platform::oauth::oauth_path("official");
    let bytes = serde_json::to_vec(&document)?;
    tokio::task::spawn_blocking(move || crate::platform::oauth::save_oauth(&owned, &bytes))
        .await??;
    Ok(document)
}

pub async fn fetch_resource(path: &Path, models: bool) -> anyhow::Result<Value> {
    let cache = Mutex::new(None);
    let (authorization, account) = headers(path, &cache).await?;
    let endpoint = if models {
        "https://chatgpt.com/backend-api/codex/models"
    } else {
        "https://chatgpt.com/backend-api/wham/usage"
    };
    let mut request = crate::platform::proxy::auto_proxy(reqwest::Client::builder())
        .build()?
        .get(endpoint)
        .header("authorization", authorization)
        .header("chatgpt-account-id", account.clone())
        .header("originator", "codex_cli_rs")
        .timeout(Duration::from_secs(20));
    if models {
        // The backend's compatibility version describes the Codex wire contract.
        request = request.query(&[("client_version", CODEX_COMPAT_VERSION)]);
    }
    let limit = if models {
        MAX_CATALOG_BYTES
    } else {
        MAX_AUTH_BYTES
    };
    let mut response = request.send().await?;
    ensure!(
        response.status().is_success(),
        "official provider endpoint returned {}",
        response.status()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            (bytes.len() + chunk.len()) as u64 <= limit,
            "official provider response exceeds its size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    let mut value: Value =
        serde_json::from_slice(&bytes).context("invalid official provider response JSON")?;
    if models {
        value["codex_mixin_account_id"] = json!(account.to_str()?);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn imported_sessions_are_not_rotated() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("external.json");
        let token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(br#"{"exp":1}"#)
        );
        let document = json!({"tokens":{"access_token":token,"refresh_token":"fixture","account_id":"account"}});
        let original = serde_json::to_vec(&document).unwrap();
        std::fs::write(&path, &original).unwrap();
        let error = headers(&path, &Mutex::new(None)).await.unwrap_err();
        assert!(error.to_string().contains("independent renewal"));
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn only_chatgpt_tokens_are_discovered() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials.json");
        assert!(!available(&path).unwrap());
        std::fs::write(&path, r#"{"OPENAI_API_KEY":"fixture"}"#).unwrap();
        assert!(!available(&path).unwrap());
        std::fs::write(
            &path,
            r#"{"tokens":{"access_token":"fixture","account_id":"account"}}"#,
        )
        .unwrap();
        assert!(available(&path).unwrap());
        std::fs::write(&path, r#"{"tokens":{}}"#).unwrap();
        assert!(available(&path).is_err());
    }
}
