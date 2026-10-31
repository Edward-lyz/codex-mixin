//! Business OAuth and authorized-user ADC are separate from consumer OAuth.
use std::path::{Path, PathBuf};
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
pub(crate) async fn headers(
    force: bool,
    owned_path: &Path,
    external_paths: &[PathBuf],
    adc_path: &Path,
    token_endpoint: &str,
) -> anyhow::Result<HeaderMap> {
    let mut selected = if let Some(value) = read_optional(owned_path).await? {
        ensure!(
            matches!(
                value.get("auth_method").and_then(Value::as_str),
                Some("gcp" | "business" | "oauth-business")
            ),
            "AGY Business owned credentials have an invalid auth method; sign in again"
        );
        Some((owned_path.to_path_buf(), value))
    } else {
        None
    };
    for path in external_paths {
        if selected.is_some() {
            break;
        }
        let Some(value) = read_optional(path).await? else {
            continue;
        };
        if matches!(
            value.get("auth_method").and_then(Value::as_str),
            Some("gcp" | "business" | "oauth-business")
        ) {
            selected = Some((path.clone(), value));
            break;
        }
    }
    let (path, document) = match selected {
        Some(source) => source,
        None => {
            let document = read_optional(adc_path).await?.context("AGY Business credentials are missing; run providers add --preset agy-business or use authorized_user ADC")?;
            (adc_path.to_path_buf(), document)
        }
    };
    let mut modified = tokio::fs::metadata(&path).await?.modified()?;
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
    if let Some(kind) = document.get("type").and_then(Value::as_str) {
        ensure!(
            kind == "authorized_user",
            "AGY Business ADC type {kind} is not supported by this build; use authorized_user ADC or the native Business OAuth session"
        );
    }
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
        let refresh = token
            .get("refresh_token")
            .and_then(Value::as_str)
            .context("AGY Business refresh token is missing; refresh the native Business login")?;
        let client_id = token.get("client_id").and_then(Value::as_str).context("AGY Business OAuth client_id is missing; refresh the native Business login or use authorized_user ADC")?;
        let client_secret = token
            .get("client_secret")
            .and_then(Value::as_str)
            .context("AGY Business OAuth client_secret is missing; use authorized_user ADC")?;
        let value = super::oauth::token_request(
            token_endpoint,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("refresh_token", refresh),
            ],
        )
        .await
        .context("refresh AGY Business OAuth")?;
        let access = value
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
        if path == owned_path {
            let mut next = token.clone();
            next["access_token"] = Value::from(access.as_str());
            next["expires_in"] = Value::from(lifetime);
            if let Some(refresh) = value.get("refresh_token").and_then(Value::as_str) {
                ensure!(!refresh.is_empty(), "Business OAuth refresh_token is empty");
                next["refresh_token"] = Value::from(refresh);
            }
            let next = super::oauth::login_document("agy-business", next)?;
            let bytes = serde_json::to_vec(&next)?;
            let owned = path.clone();
            let original = document.clone();
            modified = tokio::task::spawn_blocking(move || {
                let _lock = crate::platform::oauth::lock_oauth(&owned)?;
                let current: Value = serde_json::from_slice(&std::fs::read(&owned)?)?;
                ensure!(
                    current == original,
                    "AGY Business credentials changed during refresh; retry"
                );
                crate::platform::oauth::save_oauth(&owned, &bytes)?;
                std::fs::metadata(&owned)?
                    .modified()
                    .context("read updated Business OAuth modification time")
            })
            .await??;
        }
        (access, lifetime)
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
