use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

pub(crate) struct KiroSession {
    pub(crate) access_token: String,
    pub(crate) profile_arn: Option<String>,
    pub(crate) region: String,
}

#[derive(Deserialize, Serialize)]
struct StoredSession {
    source_path: PathBuf,
    source_modified: SystemTime,
    credentials: Value,
}

static SESSION_LOCK: Mutex<()> = Mutex::const_new(());
static CLIENT: tokio::sync::OnceCell<reqwest::Client> = tokio::sync::OnceCell::const_new();

pub(crate) async fn session(
    fallback_region: &str,
    force_refresh: bool,
) -> anyhow::Result<KiroSession> {
    let _guard = SESSION_LOCK.lock().await;
    let lock = tokio::task::spawn_blocking(|| crate::platform::oauth::lock_oauth("kiro")).await??;
    let source = crate::platform::oauth::kiro_auth_path();
    let modified = tokio::fs::metadata(&source).await
        .with_context(|| format!("Kiro credentials are missing at {}; sign in with Kiro IDE or set CODEX_MIXIN_KIRO_AUTH_FILE", source.display()))?.modified()?;
    let raw = tokio::fs::read(&source).await?;
    ensure!(
        raw.len() <= 1024 * 1024,
        "Kiro credential file exceeds 1 MiB"
    );
    let mut credentials: Value =
        serde_json::from_slice(&raw).context("invalid Kiro credential JSON")?;
    let owned = crate::platform::oauth::oauth_path("kiro");
    match tokio::fs::read(&owned).await {
        Ok(raw) => {
            let cached: StoredSession =
                serde_json::from_slice(&raw).context("invalid cached Kiro session")?;
            if cached.source_path == source && cached.source_modified == modified {
                credentials = cached.credentials;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("read cached Kiro session"),
    }
    let expiry = credentials
        .get("expiresAt")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map_or(0, |expiry| expiry.timestamp());
    if force_refresh || expiry <= chrono::Utc::now().timestamp() + 180 {
        let refresh = credentials
            .get("refreshToken")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .context("Kiro token expired and refreshToken is missing; sign in again")?
            .to_owned();
        let region = credentials
            .get("ssoRegion")
            .or_else(|| credentials.get("region"))
            .and_then(Value::as_str)
            .unwrap_or(fallback_region)
            .to_owned();
        validate_region(&region)?;
        if let Some(hash) = credentials.get("clientIdHash").and_then(Value::as_str) {
            ensure!(
                !hash.is_empty()
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
                "invalid Kiro clientIdHash"
            );
            let registration_path = source
                .parent()
                .context("Kiro auth file has no parent")?
                .join(format!("{hash}.json"));
            let registration: Value =
                serde_json::from_slice(&tokio::fs::read(registration_path).await?)?;
            credentials["clientId"] = registration["clientId"].clone();
            credentials["clientSecret"] = registration["clientSecret"].clone();
        }
        let oidc = credentials.get("clientId").and_then(Value::as_str);
        let (url, body) = if let Some(client_id) = oidc {
            let secret = credentials
                .get("clientSecret")
                .and_then(Value::as_str)
                .context("Kiro OIDC clientSecret is missing")?;
            (
                format!("https://oidc.{region}.amazonaws.com/token"),
                json!({"clientId":client_id,"clientSecret":secret,"grantType":"refresh_token","refreshToken":refresh}),
            )
        } else {
            (
                format!("https://prod.{region}.auth.desktop.kiro.dev/refreshToken"),
                json!({"refreshToken":refresh}),
            )
        };
        let response = CLIENT
            .get_or_try_init(|| async {
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(20))
                    .build()
            })
            .await
            .context("initialize native OAuth HTTP client")?
            .post(url)
            .json(&body)
            .send()
            .await
            .context("refresh Kiro session")?;
        ensure!(
            response.status().is_success(),
            "Kiro token refresh returned {}; sign in again if the session was revoked",
            response.status()
        );
        let refreshed: Value = response
            .json()
            .await
            .context("invalid Kiro refresh response")?;
        let expires = refreshed
            .get("expiresIn")
            .and_then(Value::as_u64)
            .filter(|seconds| *seconds > 0)
            .context("Kiro refresh expiry is missing")?;
        let expiry = chrono::Utc::now()
            .checked_add_signed(
                chrono::Duration::try_seconds(i64::try_from(expires)?)
                    .context("Kiro expiry is out of range")?,
            )
            .context("Kiro expiry is out of range")?;
        credentials["accessToken"] = json!(
            refreshed
                .get("accessToken")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .context("Kiro refresh accessToken is missing")?
        );
        credentials["expiresAt"] = json!(expiry.to_rfc3339());
        for field in ["refreshToken", "profileArn"] {
            if let Some(value) = refreshed.get(field) {
                credentials[field] = value.clone();
            }
        }
        let bytes = serde_json::to_vec(&StoredSession {
            source_path: source,
            source_modified: modified,
            credentials: credentials.clone(),
        })?;
        tokio::task::spawn_blocking(move || crate::platform::oauth::save_oauth(&owned, &bytes))
            .await??;
    }
    let profile_arn = credentials
        .get("profileArn")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let region = profile_arn
        .as_deref()
        .and_then(|arn| arn.split(':').nth(3))
        .filter(|region| !region.is_empty())
        .unwrap_or(fallback_region)
        .to_owned();
    validate_region(&region)?;
    let access_token = credentials
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .context("Kiro accessToken is missing")?
        .to_owned();
    drop(lock);
    Ok(KiroSession {
        access_token,
        profile_arn,
        region,
    })
}

pub(crate) fn validate_region(region: &str) -> anyhow::Result<()> {
    ensure!(
        !region.is_empty()
            && region.len() <= 64
            && region
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "invalid Kiro AWS region"
    );
    Ok(())
}
