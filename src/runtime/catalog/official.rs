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
            let client = crate::platform::proxy::auto_proxy(reqwest::Client::builder()).build()?;
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
