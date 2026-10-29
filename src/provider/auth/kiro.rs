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
    pub(crate) token_type: Option<&'static str>,
}

pub(crate) fn cli_credentials(path: &std::path::Path) -> anyhow::Result<Option<Value>> {
    if !path.is_file() {
        return Ok(None);
    }
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::from_secs(2))?;
    for (key, token_type) in [
        ("kirocli:odic:token", None),
        ("kirocli:social:token", None),
        ("kirocli:external-idp:token", Some("EXTERNAL_IDP")),
    ] {
        use rusqlite::OptionalExtension;
        let raw: Option<String> = db
            .query_row("SELECT value FROM auth_kv WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(raw) = raw else { continue };
        ensure!(
            raw.len() <= 1024 * 1024,
            "Kiro CLI credential exceeds 1 MiB"
        );
        let value: Value = serde_json::from_str(&raw).context("invalid Kiro CLI credential")?;
        if value
            .get("access_token")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            continue;
        }
        let refreshable = value
            .get("refresh_token")
            .and_then(Value::as_str)
            .is_some_and(|token| !token.is_empty());
        let valid = value
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(|expiry| chrono::DateTime::parse_from_rfc3339(expiry).ok())
            .is_some_and(|expiry| expiry.timestamp() > chrono::Utc::now().timestamp() + 180);
        if !refreshable && !valid {
            continue;
        }
        return Ok(Some(json!({
            "accessToken": value["access_token"],
            "refreshToken": value["refresh_token"],
            "expiresAt": value["expires_at"],
            "profileArn": value.get("profile_arn").or_else(|| value.get("profileArn")),
            "region": value["region"],
            "tokenType": token_type,
            "sourceKey": key,
        })));
    }
    Ok(None)
}

pub(crate) fn token_type(token: &str, external: bool) -> Option<&'static str> {
    if token.starts_with("ksk_") {
        Some("API_KEY")
    } else if external {
        Some("EXTERNAL_IDP")
    } else {
        None
    }
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
    let lock = tokio::task::spawn_blocking(|| {
        crate::platform::oauth::lock_oauth(&crate::platform::oauth::oauth_path("kiro"))
    })
    .await??;
    if let Some(path) = crate::platform::oauth::kiro_cli_auth_path() {
        let cli_path = path.clone();
        let cli = tokio::task::spawn_blocking(move || cli_credentials(&cli_path)).await??;
        if let Some(mut credentials) = cli {
            let expiry = credentials
                .get("expiresAt")
                .and_then(Value::as_str)
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map_or(0, |value| value.timestamp());
            if force_refresh || expiry <= chrono::Utc::now().timestamp() + 180 {
                tokio::task::spawn_blocking(crate::platform::oauth::refresh_kiro_cli).await??;
                credentials = tokio::task::spawn_blocking(move || cli_credentials(&path))
                    .await??
                    .context("kiro-cli login disappeared after refresh")?;
                let expiry = credentials
                    .get("expiresAt")
                    .and_then(Value::as_str)
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .context("kiro-cli login has no valid expiry after refresh")?;
                ensure!(
                    expiry.timestamp() > chrono::Utc::now().timestamp(),
                    "kiro-cli login remains expired after refresh"
                );
            }
            let access_token = credentials["accessToken"]
                .as_str()
                .context("Kiro CLI access token missing")?
                .to_owned();
            let profile_arn = credentials["profileArn"]
                .as_str()
                .filter(|v| !v.is_empty())
                .map(str::to_owned);
            let profile_region = profile_arn
                .as_deref()
                .and_then(|arn| arn.split(':').nth(3))
                .filter(|region| !region.is_empty());
            let region = profile_region
                .or_else(|| {
                    credentials["region"]
                        .as_str()
                        .filter(|region| !region.is_empty())
                })
                .unwrap_or(fallback_region);
            let region = if profile_region.is_some() {
                region
            } else {
                api_region(region)
            }
            .to_owned();
            validate_region(&region)?;
            drop(lock);
            return Ok(KiroSession {
                token_type: token_type(&access_token, credentials["tokenType"] == "EXTERNAL_IDP"),
                access_token,
                profile_arn,
                region,
            });
        }
    }
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
                crate::platform::proxy::auto_proxy(reqwest::Client::builder())
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
    let profile_region = profile_arn
        .as_deref()
        .and_then(|arn| arn.split(':').nth(3))
        .filter(|region| !region.is_empty());
    let region = profile_region
        .unwrap_or_else(|| {
            api_region(
                credentials
                    .get("ssoRegion")
                    .or_else(|| credentials.get("region"))
                    .and_then(Value::as_str)
                    .filter(|region| !region.is_empty())
                    .unwrap_or(fallback_region),
            )
        })
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
        token_type: token_type(&access_token, false),
        access_token,
        profile_arn,
        region,
    })
}

pub(crate) fn api_region(region: &str) -> &str {
    match region {
        "sa-east-1" | "us-west-1" | "us-west-2" | "us-east-2" | "ap-southeast-1"
        | "ap-southeast-2" | "ap-northeast-1" | "ap-northeast-2" | "ap-south-1" => "us-east-1",
        "eu-west-1" | "eu-west-2" | "eu-west-3" | "eu-north-1" | "eu-south-1" | "eu-south-2"
        | "eu-central-2" => "eu-central-1",
        _ => region,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cli_login_from_read_only_database_and_handles_auth_types() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.sqlite3");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute(
            "CREATE TABLE auth_kv (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        db.execute("INSERT INTO auth_kv VALUES (?1, ?2)", ["kirocli:external-idp:token", r#"{"access_token":"fixture","refresh_token":"fixture-refresh","expires_at":"2000-01-01T00:00:00Z","region":"eu-west-2"}"#]).unwrap();
        let credentials = cli_credentials(&path).unwrap().unwrap();
        assert_eq!(credentials["tokenType"], "EXTERNAL_IDP");
        assert_eq!(credentials["sourceKey"], "kirocli:external-idp:token");
        assert_eq!(api_region("eu-west-2"), "eu-central-1");
        assert_eq!(token_type("ksk_fixture", false), Some("API_KEY"));
        assert_eq!(token_type("fixture", true), Some("EXTERNAL_IDP"));
        assert_eq!(token_type("fixture", false), None);
        db.execute("DELETE FROM auth_kv", []).unwrap();
        db.execute(
            "INSERT INTO auth_kv VALUES (?1, ?2)",
            [
                "kirocli:odic:token",
                r#"{"access_token":"fixture","expires_at":"2000-01-01T00:00:00Z"}"#,
            ],
        )
        .unwrap();
        assert!(cli_credentials(&path).unwrap().is_none());
    }
}
