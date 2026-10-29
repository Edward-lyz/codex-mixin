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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, body::Bytes, routing::post};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex};

    #[tokio::test]
    async fn owned_business_refresh_persists_rotations_and_reuses_the_session() {
        let root = tempfile::tempdir().unwrap();
        let owned = root.path().join("oauth/agy-business.json");
        let native = root.path().join("native.json");
        let adc = root.path().join("adc.json");
        let initial = json!({"auth_method":"gcp","token":{"access_token":"expired","refresh_token":"original-refresh","client_id":"fixture-client","client_secret":"fixture-secret","expiry":"2020-01-01T00:00:00Z"}});
        crate::platform::oauth::save_oauth(&owned, &serde_json::to_vec(&initial).unwrap()).unwrap();
        let native_bytes = br#"{"auth_method":"gcp","token":{"access_token":"native-account","expiry":"2099-01-01T00:00:00Z"}}"#;
        std::fs::write(&native, native_bytes).unwrap();
        let received = Arc::new(StdMutex::new(Vec::<HashMap<String, String>>::new()));
        let requests = received.clone();
        let app = Router::new().route("/token", post(move |bytes: Bytes| {
            let requests = requests.clone();
            async move {
                requests.lock().unwrap().push(url::form_urlencoded::parse(&bytes).into_owned().collect());
                Json(json!({"access_token":"renewed-access","refresh_token":"rotated-refresh","expires_in":3600}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let paths = [native.clone()];
        let first = headers(false, &owned, &paths, &adc, &endpoint)
            .await
            .unwrap();
        assert_eq!(first[AUTHORIZATION], "Bearer renewed-access");
        assert_eq!(first[USER_AGENT], USER_AGENT_VALUE);
        assert_eq!(
            headers(false, &owned, &paths, &adc, &endpoint)
                .await
                .unwrap(),
            first
        );
        assert_eq!(received.lock().unwrap().len(), 1);
        headers(true, &owned, &paths, &adc, &endpoint)
            .await
            .unwrap();
        {
            let calls = received.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0]["grant_type"], "refresh_token");
            assert_eq!(calls[0]["client_id"], "fixture-client");
            assert_eq!(calls[0]["client_secret"], "fixture-secret");
            assert_eq!(calls[0]["refresh_token"], "original-refresh");
            assert_eq!(calls[1]["refresh_token"], "rotated-refresh");
        }
        let stored = read_optional(&owned).await.unwrap().unwrap();
        assert_eq!(stored["auth_method"], "gcp");
        assert_eq!(stored["token"]["refresh_token"], "rotated-refresh");
        assert_eq!(stored["token"]["client_id"], "fixture-client");
        assert_eq!(std::fs::read(native).unwrap(), native_bytes);
        assert!(!adc.exists());
        server.abort();
    }

    #[tokio::test]
    async fn imported_adc_is_read_only_and_refresh_failures_keep_owned_credentials() {
        let root = tempfile::tempdir().unwrap();
        let owned = root.path().join("oauth/agy-business.json");
        let adc = root.path().join("adc.json");
        let initial = br#"{"type":"authorized_user","refresh_token":"fixture-adc-refresh","client_id":"fixture-client","client_secret":"fixture-secret"}"#;
        std::fs::write(&adc, initial).unwrap();
        let app = Router::new()
            .route(
                "/token",
                post(|| async {
                    Json(json!({"access_token":"fixture-renewed","expires_in":3600}))
                }),
            )
            .route(
                "/denied",
                post(|| async {
                    (
                        axum::http::StatusCode::UNAUTHORIZED,
                        "fixture-private-response",
                    )
                }),
            )
            .route(
                "/invalid",
                post(|| async { Json(json!({"access_token":"fixture-renewed","expires_in":0})) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = headers(false, &owned, &[], &adc, &format!("{base}/token"))
            .await
            .unwrap();
        assert_eq!(result[AUTHORIZATION], "Bearer fixture-renewed");
        assert_eq!(std::fs::read(&adc).unwrap(), initial);
        assert!(!owned.exists());
        let document = json!({"auth_method":"gcp","token":{"refresh_token":"fixture-owned-refresh","client_id":"fixture-client","client_secret":"fixture-secret"}});
        let original = serde_json::to_vec(&document).unwrap();
        crate::platform::oauth::save_oauth(&owned, &original).unwrap();
        for route in ["denied", "invalid"] {
            let error = headers(false, &owned, &[], &adc, &format!("{base}/{route}"))
                .await
                .unwrap_err();
            assert!(!format!("{error:#}").contains("fixture-private-response"));
            assert_eq!(std::fs::read(&owned).unwrap(), original);
        }
        std::fs::remove_file(&owned).unwrap();
        std::fs::write(&adc, br#"{"type":"service_account","access_token":"fixture","expiry":"2099-01-01T00:00:00Z"}"#).unwrap();
        assert!(
            headers(false, &owned, &[], &adc, &format!("{base}/token"))
                .await
                .unwrap_err()
                .to_string()
                .contains("not supported")
        );
        server.abort();
    }

    #[tokio::test]
    async fn refresh_does_not_overwrite_a_concurrent_business_login() {
        let root = tempfile::tempdir().unwrap();
        let owned = root.path().join("oauth/agy-business.json");
        let original = json!({"auth_method":"gcp","token":{"refresh_token":"old-account","client_id":"fixture-client","client_secret":"fixture-secret"}});
        crate::platform::oauth::save_oauth(&owned, &serde_json::to_vec(&original).unwrap())
            .unwrap();
        let changed = json!({"auth_method":"gcp","token":{"access_token":"new-account","expiry":"2099-01-01T00:00:00Z"}});
        let path = owned.clone();
        let replacement = changed.clone();
        let app = Router::new().route(
            "/token",
            post(move || {
                let path = path.clone();
                let replacement = replacement.clone();
                async move {
                    crate::platform::oauth::save_oauth(
                        &path,
                        &serde_json::to_vec(&replacement).unwrap(),
                    )
                    .unwrap();
                    Json(json!({"access_token":"old-account-renewed","expires_in":3600}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let error = headers(false, &owned, &[], &root.path().join("adc.json"), &endpoint)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("credentials changed during refresh")
        );
        assert_eq!(read_optional(&owned).await.unwrap().unwrap(), changed);
        server.abort();
    }
}
