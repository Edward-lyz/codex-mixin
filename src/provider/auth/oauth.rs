//! Browser OAuth with PKCE, bounded callback lifetime and private owned storage.
use anyhow::{Context, ensure};
use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(super) const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OAUTH_LOGIN_TIMEOUT_SECONDS: u64 = 120;
const MAX_TOKEN_BYTES: usize = 64 * 1024;

type CallbackReply = tokio::sync::oneshot::Sender<Result<String, String>>;

struct CallbackState {
    state: String,
    sender: Mutex<Option<CallbackReply>>,
}
struct CallbackServer(tokio::task::JoinHandle<()>);
impl Drop for CallbackServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn callback(
    State(state): State<Arc<CallbackState>>,
    Query(query): Query<HashMap<String, String>>,
) -> (StatusCode, &'static str) {
    if query.get("state") != Some(&state.state) {
        return (StatusCode::BAD_REQUEST, "Invalid OAuth state.");
    }
    let response = match (query.get("error"), query.get("code")) {
        (Some(_), _) => Err("OAuth authorization was denied".to_owned()),
        (_, Some(code)) if !code.is_empty() => Ok(code.clone()),
        _ => return (StatusCode::BAD_REQUEST, "Authorization code is missing."),
    };
    if let Ok(mut sender) = state.sender.lock()
        && let Some(sender) = sender.take()
    {
        let _ = sender.send(response);
    }
    (
        StatusCode::OK,
        "Authorization received. Return to Codex Mixin to finish signing in.",
    )
}

pub(crate) async fn login(
    provider: &str,
    timeout_seconds: u64,
    show_url: impl Fn(&str),
) -> anyhow::Result<()> {
    ensure!(
        (1..=300).contains(&timeout_seconds),
        "OAuth timeout must be between 1 and 300 seconds"
    );
    let mut shutdown = crate::platform::ShutdownSignal::install()?;
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(timeout_seconds), login_inner(provider, show_url)) => {
            result.context("OAuth login timed out; refresh to check any completed steps")?
        }
        _ = shutdown.recv() => anyhow::bail!("OAuth login cancelled"),
    }
}

async fn login_inner(provider: &str, show_url: impl Fn(&str)) -> anyhow::Result<()> {
    let google = matches!(provider, "agy" | "agy-business");
    ensure!(
        google || provider == "official",
        "OAuth login requires agy, agy-business or official"
    );
    let (client_id, secret) = if google {
        let (id, secret) = super::agy::oauth_client()?;
        (id, Some(secret))
    } else {
        (OPENAI_CLIENT_ID.to_owned(), None)
    };
    let listener = tokio::net::TcpListener::bind(if google {
        "127.0.0.1:0"
    } else {
        "127.0.0.1:1455"
    })
    .await
    .context("bind OAuth loopback callback")?;
    let port = listener.local_addr()?.port();
    let route = if google {
        "/oauth-callback"
    } else {
        "/auth/callback"
    };
    let redirect = format!("http://localhost:{port}{route}");
    let mut verifier = [0_u8; 32];
    let mut state = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut verifier)
        .map_err(|_| anyhow::anyhow!("PKCE random generation failed"))?;
    SystemRandom::new()
        .fill(&mut state)
        .map_err(|_| anyhow::anyhow!("OAuth state generation failed"))?;
    let verifier = URL_SAFE_NO_PAD.encode(verifier);
    let challenge = URL_SAFE_NO_PAD.encode(ring::digest::digest(
        &ring::digest::SHA256,
        verifier.as_bytes(),
    ));
    let state = URL_SAFE_NO_PAD.encode(state);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let app = Router::new()
        .route(route, get(callback))
        .with_state(Arc::new(CallbackState {
            state: state.clone(),
            sender: Mutex::new(Some(sender)),
        }));
    let _server = CallbackServer(tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            tracing::warn!(%error, "OAuth callback server stopped");
        }
    }));
    let mut url = url::Url::parse(if google {
        "https://accounts.google.com/o/oauth2/v2/auth"
    } else {
        "https://auth.openai.com/oauth/authorize"
    })?;
    let scope = match provider {
        "agy-business" => {
            "https://www.googleapis.com/auth/aicode https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email"
        }
        "agy" => {
            "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email"
        }
        _ => "openid profile email offline_access",
    };
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("response_type", "code"),
        ("redirect_uri", redirect.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", state.as_str()),
        ("scope", scope),
    ]);
    if google {
        url.query_pairs_mut()
            .extend_pairs([("access_type", "offline"), ("prompt", "consent")]);
    } else {
        url.query_pairs_mut().extend_pairs([
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", "codex_cli_rs"),
        ]);
    }
    show_url(url.as_str());
    let browser_url = url.to_string();
    // The URL is also displayed, so a headless host can open it manually.
    if let Err(error) =
        tokio::task::spawn_blocking(move || crate::platform::oauth::open_oauth_url(&browser_url))
            .await?
    {
        tracing::warn!(%error, "OAuth browser could not be opened; use the displayed URL");
    }
    let code = receiver.await?.map_err(anyhow::Error::msg)?;
    let mut fields = vec![
        ("grant_type", "authorization_code"),
        ("client_id", client_id.as_str()),
        ("code", code.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("code_verifier", verifier.as_str()),
    ];
    if let Some(secret) = secret.as_deref() {
        fields.push(("client_secret", secret));
    }
    let tokens = token_request(
        if google {
            "https://oauth2.googleapis.com/token"
        } else {
            "https://auth.openai.com/oauth/token"
        },
        &fields,
    )
    .await?;
    let document = login_document(provider, tokens)?;
    let path = crate::platform::oauth::oauth_path(provider);
    let bytes = serde_json::to_vec(&document)?;
    tokio::task::spawn_blocking(move || {
        let _lock = crate::platform::oauth::lock_oauth(&path)?;
        crate::platform::oauth::save_oauth(&path, &bytes)
    })
    .await??;
    if google {
        super::agy::invalidate_agy_cache().await;
    }
    Ok(())
}

pub(super) async fn token_request(
    endpoint: &str,
    fields: &[(&str, &str)],
) -> anyhow::Result<Value> {
    let mut response = crate::platform::proxy::auto_proxy(reqwest::Client::builder())
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .post(endpoint)
        .timeout(Duration::from_secs(20))
        .form(fields)
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "OAuth token endpoint returned {}",
        response.status()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= MAX_TOKEN_BYTES,
            "OAuth token response exceeds its size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).context("invalid OAuth token response")
}

pub(super) fn jwt_claims(token: &str) -> Option<Value> {
    let bytes = URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(super) fn login_document(provider: &str, mut tokens: Value) -> anyhow::Result<Value> {
    ensure!(
        tokens["access_token"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "OAuth response is missing access_token"
    );
    ensure!(
        tokens["refresh_token"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "OAuth response is missing refresh_token"
    );
    if matches!(provider, "agy" | "agy-business") {
        let seconds = tokens["expires_in"]
            .as_i64()
            .filter(|seconds| *seconds > 0)
            .context("OAuth response is missing expiry")?;
        tokens["expiry"] = json!(
            chrono::Utc::now()
                .checked_add_signed(
                    chrono::Duration::try_seconds(seconds)
                        .context("invalid OAuth expiry duration")?
                )
                .context("OAuth expiry is out of range")?
                .to_rfc3339()
        );
        let method = if provider == "agy-business" {
            // Persist the actual client with owned credentials so refresh never
            // depends on another application's ADC or native token metadata.
            if tokens.get("client_id").is_none() && tokens.get("client_secret").is_none() {
                let (client_id, client_secret) = super::agy::oauth_client()?;
                tokens["client_id"] = json!(client_id);
                tokens["client_secret"] = json!(client_secret);
            }
            "gcp"
        } else {
            "consumer"
        };
        return Ok(json!({"auth_method":method,"token":tokens}));
    }
    let claims = tokens["id_token"]
        .as_str()
        .and_then(jwt_claims)
        .context("OpenAI OAuth response has no account claims")?;
    tokens["account_id"] = claims
        .pointer("/https://api.openai.com/auth/chatgpt_account_id")
        .and_then(Value::as_str)
        .filter(|account| !account.is_empty())
        .map(|account| json!(account))
        .context("OpenAI OAuth response is missing account_id")?;
    Ok(
        json!({"auth_mode":"chatgpt","tokens":tokens,"last_refresh":chrono::Utc::now().to_rfc3339()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn invalid_deadlines_never_open_a_browser() {
        for timeout in [0, 301] {
            let error = login("official", timeout, |_| {
                panic!("browser flow must not start")
            })
            .await
            .unwrap_err();
            assert!(error.to_string().contains("timeout"));
        }
    }

    #[tokio::test]
    async fn dropping_callback_server_releases_its_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = CallbackServer(tokio::spawn(async move {
            let _listener = listener;
            std::future::pending::<()>().await;
        }));
        drop(server);
        let listener = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(listener) = tokio::net::TcpListener::bind(address).await {
                    break listener;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(listener);
    }
    #[tokio::test]
    async fn rejects_cross_session_callback() {
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        let state = Arc::new(CallbackState {
            state: "expected".into(),
            sender: Mutex::new(Some(sender)),
        });
        assert_eq!(
            callback(
                State(state.clone()),
                Query(HashMap::from([
                    ("state".into(), "wrong".into()),
                    ("code".into(), "fixture".into())
                ]))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            callback(
                State(state),
                Query(HashMap::from([
                    ("state".into(), "expected".into()),
                    ("code".into(), "fixture".into())
                ]))
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(receiver.await.unwrap().unwrap(), "fixture");
    }
    #[test]
    fn credentials_require_refresh_and_identity() {
        assert!(login_document("agy", json!({"access_token":"fixture"})).is_err());
        assert!(
            login_document(
                "official",
                json!({"access_token":"fixture","refresh_token":"fixture"})
            )
            .is_err()
        );
    }

    #[test]
    fn business_login_documents_keep_the_refresh_client_and_session_kind() {
        let tokens = json!({"access_token":"fixture-access","refresh_token":"fixture-refresh","expires_in":3600});
        let business = login_document("agy-business", tokens.clone()).unwrap();
        assert_eq!(business["auth_method"], "gcp");
        assert_eq!(business["token"]["refresh_token"], "fixture-refresh");
        assert!(
            business["token"]["client_id"]
                .as_str()
                .unwrap()
                .ends_with(".apps.googleusercontent.com")
        );
        assert!(
            !business["token"]["client_secret"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        assert!(
            chrono::DateTime::parse_from_rfc3339(business["token"]["expiry"].as_str().unwrap())
                .unwrap()
                > chrono::Utc::now()
        );
        let consumer = login_document("agy", tokens).unwrap();
        assert_eq!(consumer["auth_method"], "consumer");
        assert!(consumer["token"].get("client_id").is_none());
        for tokens in [
            json!({"access_token":"fixture","expires_in":3600}),
            json!({"refresh_token":"fixture","expires_in":3600}),
            json!({"access_token":"fixture","refresh_token":"fixture","expires_in":0}),
        ] {
            assert!(login_document("agy-business", tokens).is_err());
        }
    }
}
