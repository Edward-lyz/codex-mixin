//! Public Codex repository catalog; no Codex executable or OAuth is required.
use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, ensure};
use serde_json::Value;

pub const OFFICIAL_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/openai/codex/main/codex-rs/models-manager/models.json";
const MAX_CATALOG_BYTES: usize = 8 * 1024 * 1024;
pub const OFFICIAL_CATALOG_TIMEOUT: Duration = Duration::from_secs(10);
static CATALOG_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub async fn fetch_catalog(
    source_url: &str,
    timeout: Duration,
    cached: Option<&Value>,
) -> anyhow::Result<Value> {
    let client = match CATALOG_CLIENT.get() {
        Some(client) => client.clone(),
        None => {
            let client = reqwest::Client::builder().build()?;
            let _ = CATALOG_CLIENT.set(client.clone());
            client
        }
    };
    let mut request = client.get(source_url).timeout(timeout).header(
        reqwest::header::USER_AGENT,
        concat!("codex-mixin/", env!("CARGO_PKG_VERSION")),
    );
    let cached = cached.filter(|catalog| {
        catalog["codex_mixin_http"]["url"].as_str() == Some(source_url)
            && (catalog["codex_mixin_http"]["etag"].is_string()
                || catalog["codex_mixin_http"]["last_modified"].is_string())
            && validate_catalog(catalog).is_ok()
    });
    if let Some(catalog) = cached {
        if let Some(etag) = catalog["codex_mixin_http"]["etag"].as_str() {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        } else if let Some(modified) = catalog["codex_mixin_http"]["last_modified"].as_str() {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, modified);
        }
    }
    let response = request
        .send()
        .await
        .context("download public Codex repository models.json")?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        let cached = cached.context("repository returned 304 without a valid matching catalog")?;
        tracing::info!(
            event = "official_catalog_not_modified",
            source_url,
            status = 304,
            "official catalog unchanged; cached body reused"
        );
        return Ok(cached.clone());
    }
    let mut validators = serde_json::json!({
        "url": source_url,
        "etag": response.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()),
        "last_modified": response.headers().get(reqwest::header::LAST_MODIFIED).and_then(|v| v.to_str().ok()),
    });
    if response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|directive| directive.trim().eq_ignore_ascii_case("no-store"))
        })
    {
        validators = Value::Null;
    }
    let mut response = response
        .error_for_status()
        .context("Codex repository models.json HTTP status")?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("read Codex repository models.json")?
    {
        ensure!(
            bytes.len() + chunk.len() <= MAX_CATALOG_BYTES,
            "Codex repository model catalog exceeds 8 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    let mut catalog: Value = serde_json::from_slice(&bytes)
        .context("Codex repository model catalog contains invalid JSON")?;
    validate_catalog(&catalog)?;
    catalog["codex_mixin_http"] = validators;
    tracing::info!(
        event = "official_catalog_downloaded",
        source_url,
        model_count = catalog["models"].as_array().map_or(0, Vec::len),
        "official model catalog downloaded from public repository"
    );
    Ok(catalog)
}

pub fn validate_catalog(catalog: &Value) -> anyhow::Result<()> {
    let models = catalog
        .get("models")
        .and_then(Value::as_array)
        .context("Codex repository model catalog has no models array")?;
    ensure!(
        !models.is_empty(),
        "Codex repository model catalog is empty"
    );
    let mut slugs = HashSet::with_capacity(models.len());
    for model in models {
        let slug = model
            .get("slug")
            .and_then(Value::as_str)
            .context("Codex repository model is missing slug")?;
        ensure!(
            !slug.is_empty() && slug.trim() == slug,
            "Codex repository model has an invalid slug"
        );
        ensure!(
            slugs.insert(slug),
            "duplicate Codex repository model slug: {slug}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn conditional_catalog_reuses_body_and_replaces_changed_content() {
        use axum::response::IntoResponse;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let count = Arc::new(AtomicUsize::new(0));
        let requests = count.clone();
        let app = axum::Router::new().route(
            "/models.json",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let requests = requests.clone();
                async move {
                    let phase = requests.fetch_add(1, Ordering::SeqCst);
                    let validator = headers
                        .get("if-none-match")
                        .and_then(|value| value.to_str().ok());
                    assert_eq!(validator, if phase == 0 { None } else { Some("\"v1\"") });
                    if phase == 1 {
                        return axum::http::StatusCode::NOT_MODIFIED.into_response();
                    }
                    let slug = if phase == 0 { "model-a" } else { "model-b" };
                    (
                        [("etag", "\"v1\"")],
                        axum::Json(json!({"models":[{"slug":slug}]})),
                    )
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let url = format!("http://{address}/models.json");
        let first = fetch_catalog(&url, Duration::from_secs(2), None)
            .await
            .unwrap();
        let unchanged = fetch_catalog(&url, Duration::from_secs(2), Some(&first))
            .await
            .unwrap();
        assert_eq!(unchanged, first);
        let changed = fetch_catalog(&url, Duration::from_secs(2), Some(&unchanged))
            .await
            .unwrap();
        assert_eq!(changed["models"][0]["slug"], "model-b");
        assert_eq!(count.load(Ordering::SeqCst), 3);
        server.abort();
    }

    #[tokio::test]
    async fn rejects_304_without_matching_cache() {
        let app = axum::Router::new().route(
            "/models.json",
            axum::routing::get(|| async { axum::http::StatusCode::NOT_MODIFIED }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let error = fetch_catalog(&format!("http://{address}/models.json"), Duration::from_secs(2),
            Some(&json!({"models":[{"slug":"old"}],"codex_mixin_http":{"url":"https://other.test","etag":"v1"}})))
            .await.unwrap_err();
        assert!(error.to_string().contains("304 without"));
        server.abort();
    }

    #[tokio::test]
    async fn last_modified_revalidation_and_no_store() {
        use axum::response::IntoResponse;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let app = axum::Router::new().route(
            "/models.json",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let count = count.clone();
                async move {
                    let phase = count.fetch_add(1, Ordering::SeqCst);
                    assert!(headers.get("if-none-match").is_none());
                    assert_eq!(
                        headers
                            .get("if-modified-since")
                            .and_then(|v| v.to_str().ok()),
                        if phase == 1 {
                            Some("Wed, 30 Sep 2026 00:00:00 GMT")
                        } else {
                            None
                        }
                    );
                    let mut response =
                        axum::Json(json!({"models":[{"slug":"model-a"}]})).into_response();
                    response.headers_mut().insert(
                        "last-modified",
                        "Wed, 30 Sep 2026 00:00:00 GMT".parse().unwrap(),
                    );
                    if phase == 1 {
                        response
                            .headers_mut()
                            .insert("cache-control", "no-store".parse().unwrap());
                    }
                    response
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let url = format!("http://{address}/models.json");
        let first = fetch_catalog(&url, Duration::from_secs(2), None)
            .await
            .unwrap();
        let second = fetch_catalog(&url, Duration::from_secs(2), Some(&first))
            .await
            .unwrap();
        assert!(second["codex_mixin_http"].is_null());
        fetch_catalog(&url, Duration::from_secs(2), Some(&second))
            .await
            .unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        server.abort();
    }

    #[test]
    fn validates_catalog_before_replacing_cache() {
        for catalog in [
            json!({}),
            json!({"models":[]}),
            json!({"models":[{}]}),
            json!({"models":[{"slug":""}]}),
            json!({"models":[{"slug":"gpt-test"},{"slug":"gpt-test"}]}),
        ] {
            assert!(validate_catalog(&catalog).is_err());
        }
        assert!(
            validate_catalog(&json!({"models":[{"slug":"gpt-test","visibility":"hide"}]})).is_ok()
        );
    }

    #[tokio::test]
    async fn public_catalog_fetch_sends_no_account_headers_or_version_query() {
        let app = axum::Router::new().route(
            "/models.json",
            axum::routing::get(
                |headers: axum::http::HeaderMap, uri: axum::http::Uri| async move {
                    assert!(headers.get("authorization").is_none());
                    assert!(headers.get("chatgpt-account-id").is_none());
                    assert!(uri.query().is_none());
                    axum::Json(json!({"models":[{"slug":"gpt-test","context_window":272000}]}))
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let catalog = fetch_catalog(
            &format!("http://{address}/models.json"),
            Duration::from_secs(2),
            None,
        )
        .await
        .unwrap();
        assert_eq!(catalog["models"][0]["context_window"], 272000);
        server.abort();
    }

    #[tokio::test]
    async fn download_timeout_also_bounds_response_body() {
        let app = axum::Router::new().route(
            "/models.json",
            axum::routing::get(|| async {
                axum::body::Body::from_stream(futures_util::stream::pending::<
                    Result<bytes::Bytes, std::convert::Infallible>,
                >())
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let source_url = format!("http://{address}/models.json");
        let fetch = fetch_catalog(&source_url, Duration::from_millis(50), None);
        let error = tokio::time::timeout(Duration::from_millis(500), fetch)
            .await
            .unwrap()
            .unwrap_err();
        assert!(format!("{error:#}").contains("timed out"));
        server.abort();
    }
}
