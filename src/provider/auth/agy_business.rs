//! Business OAuth and authorized-user ADC are separate from consumer OAuth.
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, bail, ensure};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;
use tokio::sync::Mutex;

pub(crate) const USER_AGENT_VALUE: &str = "antigravity/cli/1.3.5 (aidev_client; auth_method=gcp)";
struct CachedSession {
    path: PathBuf,
    modified: SystemTime,
    headers: HeaderMap,
    expires: Instant,
}
static CACHE: Mutex<Option<CachedSession>> = Mutex::const_new(None);
static CLIENT: tokio::sync::OnceCell<reqwest::Client> = tokio::sync::OnceCell::const_new();

pub(crate) async fn headers(force: bool) -> anyhow::Result<HeaderMap> {
    let mut selected = None;
    for path in crate::platform::oauth::agy_external_token_paths() {
        let Some(value) = read_optional(&path).await? else {
            continue;
        };
        if matches!(
            value.get("auth_method").and_then(Value::as_str),
            Some("gcp" | "business" | "oauth-business")
        ) {
            selected = Some((path, value));
            break;
        }
    }
    let (path, document) = match selected {
        Some(source) => source,
        None => {
            let path = crate::platform::oauth::google_adc_path();
            let document = read_optional(&path).await?.context("AGY Business credentials are missing; sign in with agy Business or run gcloud auth application-default login --project PROJECT_ID")?;
            (path, document)
        }
    };
    let modified = tokio::fs::metadata(&path).await?.modified()?;
    {
        let cache = CACHE.lock().await;
        if !force
            && let Some(cached) = cache.as_ref().filter(|cached| {
                cached.path == path
                    && cached.modified == modified
                    && cached.expires > Instant::now()
            })
        {
            return Ok(cached.headers.clone());
        }
    }
    let token = document.get("token").unwrap_or(&document);
    let lifetime = token
        .get("expiry")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map_or(0, |expiry| {
            (expiry.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64
        });
    let (access_token, lifetime) = if !force && lifetime > 180 {
        (
            token
                .get("access_token")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .context("AGY Business access token is missing")?
                .to_owned(),
            lifetime,
        )
    } else {
        if let Some(kind) = document.get("type").and_then(Value::as_str) {
            ensure!(
                kind == "authorized_user",
                "AGY Business ADC type {kind} is not supported by this build; use authorized_user ADC or the native Business OAuth session"
            );
        }
        let refresh = token
            .get("refresh_token")
            .and_then(Value::as_str)
            .context("AGY Business refresh token is missing; refresh the native Business login")?;
        let client_id = token.get("client_id").and_then(Value::as_str).context("AGY Business OAuth client_id is missing; refresh the native Business login or use authorized_user ADC")?;
        let client_secret = token
            .get("client_secret")
            .and_then(Value::as_str)
            .context("AGY Business OAuth client_secret is missing; use authorized_user ADC")?;
        let response = CLIENT
            .get_or_try_init(|| async {
                crate::platform::proxy::auto_proxy(reqwest::Client::builder())
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(20))
                    .build()
            })
            .await
            .context("initialize native OAuth HTTP client")?
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("refresh_token", refresh),
            ])
            .send()
            .await
            .context("refresh AGY Business OAuth")?;
        ensure!(
            response.status().is_success(),
            "AGY Business OAuth refresh returned {}",
            response.status()
        );
        let value = super::super::agy::bounded_json(response, "Business OAuth refresh").await?;
        let token = value
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .context("Business OAuth refresh access_token is missing")?
            .to_owned();
        let lifetime = value
            .get("expires_in")
            .and_then(Value::as_u64)
            .filter(|seconds| *seconds > 0)
            .context("Business OAuth refresh expires_in is missing")?;
        (token, lifetime)
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}"))?,
    );
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    *CACHE.lock().await = Some(CachedSession {
        path,
        modified,
        headers: headers.clone(),
        expires: Instant::now()
            .checked_add(Duration::from_secs(lifetime.saturating_sub(180)))
            .context("Business OAuth expiry is out of range")?,
    });
    Ok(headers)
}

pub(crate) async fn read_optional(path: &std::path::Path) -> anyhow::Result<Option<Value>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            ensure!(
                bytes.len() <= 1024 * 1024,
                "AGY settings or credential file exceeds 1 MiB: {}",
                path.display()
            );
            Ok(Some(
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse {}", path.display()))?,
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => bail!("read {}: {error}", path.display()),
    }
}
