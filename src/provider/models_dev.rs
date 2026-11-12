//! The models.dev catalog: one place that fetches, caches, and bundles it.
//!
//! The resolver reads model limits and capabilities from it and pricing reads
//! list prices. A full snapshot taken at build time
//! (`scripts/update_models_dev_snapshot.sh`) answers until a local cache
//! exists, so offline and intranet installs still get models.dev data.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const BUNDLED_CATALOG: &[u8] = include_bytes!("models_dev_api.json.zst");
const MAX_BUNDLED_BYTES: u64 = 64 * 1024 * 1024;

pub const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
const MODELS_DEV_REFRESH_INTERVAL: Duration = Duration::from_secs(3 * 60 * 60);
const MODELS_DEV_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

static MODELS_DEV_CATALOG_CACHE: LazyLock<tokio::sync::Mutex<Option<CachedModelsDevCatalog>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

/// Holds the parsed catalog only while a discovery pass uses it; the parsed
/// form is several MB, and the validated disk copy re-parses in milliseconds.
struct CachedModelsDevCatalog {
    catalog: Weak<ModelsDevCatalog>,
    /// Keeps a catalog without a disk copy (the bundled snapshot after a
    /// failed first fetch) alive until its retry deadline.
    pinned: Option<Arc<ModelsDevCatalog>>,
    cache_path: PathBuf,
    source_url: String,
    refresh_after: tokio::time::Instant,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelsDevHttpCache {
    source_url: String,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    last_modified: Option<String>,
    checked_at_unix_seconds: u64,
    #[serde(default)]
    retry_after_unix_seconds: Option<u64>,
    catalog_len: u64,
    catalog_modified_unix_nanos: u64,
}

#[derive(Clone, Copy, Debug)]
struct ModelsDevCacheStamp {
    len: u64,
    modified_unix_nanos: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelsDevCatalog {
    #[serde(flatten)]
    pub(super) providers: BTreeMap<String, ModelsDevProvider>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ModelsDevProvider {
    #[serde(default)]
    pub(super) models: BTreeMap<String, ModelsDevModel>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ModelsDevModel {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) attachment: Option<bool>,
    #[serde(default)]
    pub(super) reasoning: Option<bool>,
    #[serde(default)]
    pub(super) modalities: Option<ModelsDevModalities>,
    #[serde(default)]
    pub(super) limit: Option<ModelsDevLimit>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ModelsDevModalities {
    #[serde(default)]
    pub(super) input: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ModelsDevLimit {
    #[serde(default)]
    pub(super) context: Option<u64>,
    #[serde(default)]
    pub(super) input: Option<u64>,
    #[serde(default)]
    pub(super) output: Option<u64>,
}

/// The catalog file every reader uses: the file named by
/// `CODEX_GATEWAY_MODEL_METADATA`, else the shared cache.
pub fn metadata_catalog_path() -> PathBuf {
    match std::env::var("CODEX_GATEWAY_MODEL_METADATA") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => default_metadata_cache_path(),
    }
}

/// The snapshot bundled at build time, decompressed.
pub(crate) fn bundled_catalog_json() -> anyhow::Result<Vec<u8>> {
    let mut raw = Vec::new();
    zstd::stream::read::Decoder::new(BUNDLED_CATALOG)?
        .take(MAX_BUNDLED_BYTES)
        .read_to_end(&mut raw)
        .context("bundled models.dev snapshot is unreadable")?;
    Ok(raw)
}

fn bundled_catalog() -> anyhow::Result<Arc<ModelsDevCatalog>> {
    let raw = bundled_catalog_json()?;
    let catalog = serde_json::from_slice(&raw)
        .context("bundled models.dev snapshot has an unexpected shape")?;
    Ok(Arc::new(catalog))
}

pub fn default_metadata_cache_path() -> PathBuf {
    if let Ok(path) = std::env::var("CODEX_GATEWAY_MODEL_METADATA_CACHE")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    codex_mixin_home().join("models_dev_api.json")
}

fn codex_mixin_home() -> PathBuf {
    crate::platform::home_dir().join(".codex-mixin")
}

/// Fetch the complete models.dev catalog.
///
/// Some provider catalogs return IDs only (OpenCode Go, DeepSeek) or omit
/// capabilities entirely (AWS Bedrock control plane); models.dev supplies the
/// missing limits and capability flags for those.
pub(crate) async fn fetch_models_dev_catalog(
    client: &Client,
) -> anyhow::Result<Arc<ModelsDevCatalog>> {
    if let Ok(path) = std::env::var("CODEX_GATEWAY_MODEL_METADATA")
        && !path.is_empty()
    {
        return load_models_dev_catalog(
            client,
            "",
            Path::new(&path),
            None,
            &MODELS_DEV_CATALOG_CACHE,
        )
        .await;
    }
    let source_url = std::env::var("CODEX_GATEWAY_MODEL_METADATA_URL")
        .unwrap_or_else(|_| MODELS_DEV_API_URL.to_owned());
    load_models_dev_catalog(
        client,
        &source_url,
        &default_metadata_cache_path(),
        Some(MODELS_DEV_REFRESH_INTERVAL),
        &MODELS_DEV_CATALOG_CACHE,
    )
    .await
}

async fn load_models_dev_catalog(
    client: &Client,
    source_url: &str,
    cache_path: &Path,
    refresh_interval: Option<Duration>,
    cache: &tokio::sync::Mutex<Option<CachedModelsDevCatalog>>,
) -> anyhow::Result<Arc<ModelsDevCatalog>> {
    let now = tokio::time::Instant::now();
    let mut cached = cache.lock().await;
    let cache_matches = |entry: &CachedModelsDevCatalog| {
        entry.cache_path == cache_path && entry.source_url == source_url
    };
    if let Some(entry) = cached.as_ref().filter(|entry| cache_matches(entry))
        && (refresh_interval.is_none() || now < entry.refresh_after)
        && let Some(catalog) = entry.pinned.clone().or_else(|| entry.catalog.upgrade())
    {
        return Ok(catalog);
    }

    let mut stale_catalog = cached
        .as_ref()
        .filter(|entry| cache_matches(entry))
        .and_then(|entry| entry.catalog.upgrade());
    let mut http_cache = None;
    if tokio::fs::try_exists(cache_path).await? {
        match read_models_dev_catalog(cache_path).await {
            Ok(catalog) => {
                let Some(refresh_interval) = refresh_interval else {
                    *cached = Some(CachedModelsDevCatalog {
                        catalog: Arc::downgrade(&catalog),
                        pinned: None,
                        cache_path: cache_path.to_path_buf(),
                        source_url: source_url.to_owned(),
                        refresh_after: now,
                    });
                    return Ok(catalog);
                };
                match read_models_dev_http_cache(cache_path, source_url).await {
                    Ok(Some(metadata)) => {
                        let refresh_delay = metadata.refresh_delay(refresh_interval);
                        if !refresh_delay.is_zero() {
                            *cached = Some(CachedModelsDevCatalog {
                                catalog: Arc::downgrade(&catalog),
                                pinned: None,
                                cache_path: cache_path.to_path_buf(),
                                source_url: source_url.to_owned(),
                                refresh_after: now + refresh_delay,
                            });
                            return Ok(catalog);
                        }
                        http_cache = Some(metadata);
                    }
                    Ok(None) => {}
                    Err(error) => tracing::warn!(
                        path = %models_dev_http_cache_path(cache_path).display(),
                        error = %format!("{error:#}"),
                        "ignoring invalid models.dev HTTP cache before unconditional refresh"
                    ),
                }
                stale_catalog = Some(catalog);
            }
            Err(error) if refresh_interval.is_some() => tracing::warn!(
                path = %cache_path.display(),
                error = %format!("{error:#}"),
                "ignoring invalid models.dev cache before remote refresh"
            ),
            Err(error) => return Err(error),
        }
    }
    let Some(refresh_interval) = refresh_interval else {
        anyhow::bail!(
            "configured models.dev metadata file does not exist: {}",
            cache_path.display()
        );
    };
    let (catalog, refresh_after) = refresh_models_dev_catalog(
        client,
        source_url,
        cache_path,
        refresh_interval,
        stale_catalog,
        http_cache,
    )
    .await?;
    let pinned =
        (!tokio::fs::try_exists(cache_path).await.unwrap_or(false)).then(|| Arc::clone(&catalog));
    *cached = Some(CachedModelsDevCatalog {
        catalog: Arc::downgrade(&catalog),
        pinned,
        cache_path: cache_path.to_path_buf(),
        source_url: source_url.to_owned(),
        refresh_after,
    });
    Ok(catalog)
}

async fn refresh_models_dev_catalog(
    client: &Client,
    source_url: &str,
    cache_path: &Path,
    refresh_interval: Duration,
    stale_catalog: Option<Arc<ModelsDevCatalog>>,
    http_cache: Option<ModelsDevHttpCache>,
) -> anyhow::Result<(Arc<ModelsDevCatalog>, tokio::time::Instant)> {
    let fetched =
        match fetch_models_dev_catalog_from(client, source_url, http_cache.as_ref()).await {
            Ok(fetched) => fetched,
            Err(error) => {
                let catalog = match stale_catalog {
                    Some(catalog) => catalog,
                    None => {
                        let bundled = match bundled_catalog() {
                            Ok(bundled) => bundled,
                            Err(bundled_error) => {
                                return Err(error
                                    .context(format!("no bundled fallback: {bundled_error:#}")));
                            }
                        };
                        tracing::warn!(
                            error = %format!("{error:#}"),
                            retry_seconds = MODELS_DEV_RETRY_INTERVAL.as_secs(),
                            "models.dev fetch failed; using the bundled snapshot"
                        );
                        return Ok((
                            bundled,
                            tokio::time::Instant::now() + MODELS_DEV_RETRY_INTERVAL,
                        ));
                    }
                };
                persist_models_dev_retry(cache_path, source_url, http_cache).await;
                tracing::warn!(
                    error = %format!("{error:#}"),
                    retry_seconds = MODELS_DEV_RETRY_INTERVAL.as_secs(),
                    "models.dev refresh failed; using stale local cache"
                );
                return Ok((
                    catalog,
                    tokio::time::Instant::now() + MODELS_DEV_RETRY_INTERVAL,
                ));
            }
        };
    let catalog = match fetched {
        ModelsDevFetch::Modified {
            catalog,
            body,
            etag,
            last_modified,
        } => {
            persist_modified_catalog(cache_path, source_url, body, etag, last_modified).await;
            catalog
        }
        ModelsDevFetch::NotModified {
            etag,
            last_modified,
        } => {
            let catalog = stale_catalog
                .ok_or_else(|| anyhow!("models.dev returned 304 without a cached catalog"))?;
            let metadata = http_cache
                .ok_or_else(|| anyhow!("models.dev returned 304 without cache validators"))?;
            persist_not_modified(cache_path, metadata, etag, last_modified).await;
            catalog
        }
    };
    Ok((catalog, tokio::time::Instant::now() + refresh_interval))
}

async fn read_models_dev_catalog(path: &Path) -> anyhow::Result<Arc<ModelsDevCatalog>> {
    let body = tokio::fs::read(path)
        .await
        .with_context(|| format!("read models.dev cache {}", path.display()))?;
    serde_json::from_slice(&body)
        .map(Arc::new)
        .context("models.dev cache contains invalid JSON")
}

enum ModelsDevFetch {
    Modified {
        catalog: Arc<ModelsDevCatalog>,
        body: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    NotModified {
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

async fn fetch_models_dev_catalog_from(
    client: &Client,
    source_url: &str,
    http_cache: Option<&ModelsDevHttpCache>,
) -> anyhow::Result<ModelsDevFetch> {
    let mut request = client
        .get(source_url)
        .header(reqwest::header::USER_AGENT, "codex-mixin");
    if let Some(metadata) = http_cache {
        if let Some(etag) = metadata.etag.as_deref() {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = metadata.last_modified.as_deref() {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, last_modified);
        }
    }
    let response = request
        .send()
        .await
        .context("failed to request the models.dev catalog")?;
    let status = response.status();
    let etag = response_header(&response, reqwest::header::ETAG);
    let last_modified = response_header(&response, reqwest::header::LAST_MODIFIED);
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(ModelsDevFetch::NotModified {
            etag,
            last_modified,
        });
    }
    if !status.is_success() {
        return Err(anyhow!("models.dev catalog returned {status}"));
    }
    let body = response
        .bytes()
        .await
        .context("failed to read models.dev catalog body")?;
    let catalog = serde_json::from_slice(&body)
        .map(Arc::new)
        .context("models.dev catalog returned invalid JSON")?;
    Ok(ModelsDevFetch::Modified {
        catalog,
        body: body.to_vec(),
        etag,
        last_modified,
    })
}

fn response_header(
    response: &reqwest::Response,
    name: reqwest::header::HeaderName,
) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

impl ModelsDevHttpCache {
    fn refresh_delay(&self, refresh_interval: Duration) -> Duration {
        let now = unix_seconds();
        if let Some(retry_after) = self.retry_after_unix_seconds
            && retry_after > now
        {
            return Duration::from_secs(retry_after - now);
        }
        refresh_interval.saturating_sub(Duration::from_secs(
            now.saturating_sub(self.checked_at_unix_seconds),
        ))
    }

    fn matches(&self, source_url: &str, stamp: ModelsDevCacheStamp) -> bool {
        self.source_url == source_url
            && self.catalog_len == stamp.len
            && self.catalog_modified_unix_nanos == stamp.modified_unix_nanos
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn models_dev_http_cache_path(cache_path: &Path) -> PathBuf {
    cache_path.with_extension("http.json")
}

async fn models_dev_cache_stamp(cache_path: &Path) -> anyhow::Result<ModelsDevCacheStamp> {
    let metadata = tokio::fs::metadata(cache_path)
        .await
        .with_context(|| format!("inspect models.dev cache {}", cache_path.display()))?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .context("models.dev cache modification time predates the Unix epoch")?;
    let modified_unix_nanos = u64::try_from(modified.as_nanos())
        .context("models.dev cache modification time is out of range")?;
    Ok(ModelsDevCacheStamp {
        len: metadata.len(),
        modified_unix_nanos,
    })
}

async fn read_models_dev_http_cache(
    cache_path: &Path,
    source_url: &str,
) -> anyhow::Result<Option<ModelsDevHttpCache>> {
    let http_cache_path = models_dev_http_cache_path(cache_path);
    if !tokio::fs::try_exists(&http_cache_path).await? {
        return Ok(None);
    }
    let body = tokio::fs::read(&http_cache_path)
        .await
        .with_context(|| format!("read models.dev HTTP cache {}", http_cache_path.display()))?;
    let metadata: ModelsDevHttpCache =
        serde_json::from_slice(&body).context("models.dev HTTP cache contains invalid JSON")?;
    let stamp = models_dev_cache_stamp(cache_path).await?;
    if !metadata.matches(source_url, stamp) {
        anyhow::bail!("models.dev HTTP cache does not match the catalog file");
    }
    Ok(Some(metadata))
}

async fn persist_modified_catalog(
    cache_path: &Path,
    source_url: &str,
    body: Vec<u8>,
    etag: Option<String>,
    last_modified: Option<String>,
) {
    if let Err(error) = write_models_dev_cache(cache_path, body).await {
        tracing::warn!(
            path = %cache_path.display(),
            error = %format!("{error:#}"),
            "models.dev catalog is cached in memory but could not be persisted"
        );
        return;
    }
    let stamp = match models_dev_cache_stamp(cache_path).await {
        Ok(stamp) => stamp,
        Err(error) => {
            tracing::warn!(
                path = %cache_path.display(),
                error = %format!("{error:#}"),
                "could not inspect the persisted models.dev catalog"
            );
            return;
        }
    };
    let metadata = ModelsDevHttpCache {
        source_url: source_url.to_owned(),
        etag,
        last_modified,
        checked_at_unix_seconds: unix_seconds(),
        retry_after_unix_seconds: None,
        catalog_len: stamp.len,
        catalog_modified_unix_nanos: stamp.modified_unix_nanos,
    };
    persist_models_dev_http_cache(cache_path, &metadata).await;
}

async fn persist_not_modified(
    cache_path: &Path,
    mut metadata: ModelsDevHttpCache,
    etag: Option<String>,
    last_modified: Option<String>,
) {
    if etag.is_some() {
        metadata.etag = etag;
    }
    if last_modified.is_some() {
        metadata.last_modified = last_modified;
    }
    metadata.checked_at_unix_seconds = unix_seconds();
    metadata.retry_after_unix_seconds = None;
    persist_models_dev_http_cache(cache_path, &metadata).await;
}

async fn persist_models_dev_retry(
    cache_path: &Path,
    source_url: &str,
    http_cache: Option<ModelsDevHttpCache>,
) {
    let mut metadata = if let Some(metadata) = http_cache {
        metadata
    } else {
        let Ok(stamp) = models_dev_cache_stamp(cache_path).await else {
            return;
        };
        ModelsDevHttpCache {
            source_url: source_url.to_owned(),
            etag: None,
            last_modified: None,
            checked_at_unix_seconds: 0,
            retry_after_unix_seconds: None,
            catalog_len: stamp.len,
            catalog_modified_unix_nanos: stamp.modified_unix_nanos,
        }
    };
    metadata.retry_after_unix_seconds = Some(unix_seconds() + MODELS_DEV_RETRY_INTERVAL.as_secs());
    persist_models_dev_http_cache(cache_path, &metadata).await;
}

async fn persist_models_dev_http_cache(cache_path: &Path, metadata: &ModelsDevHttpCache) {
    let path = models_dev_http_cache_path(cache_path);
    let body = match serde_json::to_vec_pretty(metadata) {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(error = %error, "could not serialize models.dev HTTP cache");
            return;
        }
    };
    if let Err(error) = write_models_dev_cache(&path, body).await {
        tracing::warn!(
            path = %path.display(),
            error = %format!("{error:#}"),
            "could not persist models.dev HTTP cache"
        );
    }
}

async fn write_models_dev_cache(cache_path: &Path, body: Vec<u8>) -> anyhow::Result<()> {
    let cache_path = cache_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        crate::platform::write_atomic_if_changed(&cache_path, &body)
    })
    .await
    .context("models.dev cache writer task failed")??;
    Ok(())
}
