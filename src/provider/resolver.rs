//! Unified model metadata resolution.
//!
//! One resolver answers "what are this model's limits and modalities" from
//! two layered sources: a cached models.dev catalog, then built-in family
//! rules as the offline fallback. models.dev is also the capability source
//! for providers whose own catalog only returns model ids (OpenCode Go
//! today).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::ProviderModel;

pub const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
const MODELS_DEV_REFRESH_INTERVAL: Duration = Duration::from_secs(3 * 60 * 60);
const MODELS_DEV_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

static MODELS_DEV_CATALOG_CACHE: LazyLock<tokio::sync::Mutex<Option<CachedModelsDevCatalog>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

struct CachedModelsDevCatalog {
    catalog: Arc<ModelsDevCatalog>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelMetadata {
    pub context_window: u64,
    pub max_output_tokens: Option<u64>,
    pub input_modalities: Vec<String>,
    pub supports_image: Option<bool>,
    pub supports_thinking: Option<bool>,
    pub source: String,
}

#[derive(Clone, Debug, Default)]
pub struct MetadataResolver {
    entries: Vec<MetadataEntry>,
    token_index: HashMap<String, Vec<usize>>,
}

#[derive(Clone, Debug)]
struct MetadataEntry {
    key: String,
    token_variants: Vec<Vec<String>>,
    metadata: ModelMetadata,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelsDevCatalog {
    #[serde(flatten)]
    providers: BTreeMap<String, ModelsDevProvider>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevProvider {
    #[serde(default)]
    models: BTreeMap<String, ModelsDevModel>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModel {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    attachment: Option<bool>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    modalities: Option<ModelsDevModalities>,
    #[serde(default)]
    limit: Option<ModelsDevLimit>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModalities {
    #[serde(default)]
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevLimit {
    #[serde(default)]
    context: Option<u64>,
    #[serde(default)]
    input: Option<u64>,
    #[serde(default)]
    output: Option<u64>,
}

impl MetadataResolver {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_json(value: &Value) -> anyhow::Result<Self> {
        let catalog: ModelsDevCatalog = serde_json::from_value(value.clone())
            .context("models.dev catalog has an unexpected shape")?;
        Ok(Self::from_catalog(&catalog))
    }

    pub(crate) fn from_catalog(catalog: &ModelsDevCatalog) -> Self {
        let mut entries = Vec::new();
        for (provider_id, provider) in &catalog.providers {
            for (model_id, model) in &provider.models {
                let Some(context_window) = model
                    .limit
                    .as_ref()
                    .and_then(|limit| limit.context.or(limit.input))
                else {
                    continue;
                };
                let key = format!("{provider_id}/{model_id}");
                entries.push(MetadataEntry {
                    // Match on the bare model id; the provider prefix only
                    // breaks ties through provider_priority.
                    token_variants: token_variants(model_id),
                    metadata: ModelMetadata {
                        context_window,
                        max_output_tokens: model.limit.as_ref().and_then(|limit| limit.output),
                        input_modalities: models_dev_input_modalities(model),
                        supports_image: model_supports_image(model),
                        supports_thinking: model.reasoning,
                        source: format!("models.dev:{key}"),
                    },
                    key,
                });
            }
        }
        Self::from_entries(entries)
    }

    pub fn from_json_file(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let value = serde_json::from_str(&raw)?;
        Self::from_json(&value)
    }

    pub fn from_default_files() -> anyhow::Result<Self> {
        if let Ok(path) = std::env::var("CODEX_GATEWAY_MODEL_METADATA")
            && !path.is_empty()
        {
            return Self::from_json_file(Path::new(&path));
        }
        let cache_path = default_metadata_cache_path();
        if cache_path.exists() {
            return Self::from_json_file(&cache_path);
        }
        Ok(Self::empty())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn resolve(&self, model: &str, default_context_window: u64) -> ModelMetadata {
        let query_variants = token_variants(model);
        if let Some(entry) = self.best_match(&query_variants) {
            return entry.metadata.clone();
        }
        builtin_metadata(model, default_context_window)
    }

    /// Fuzzy models.dev lookup without the built-in family fallback: None
    /// means models.dev has no plausible match for this model id.
    pub fn lookup(&self, model: &str) -> Option<&ModelMetadata> {
        self.best_match(&token_variants(model))
            .map(|entry| &entry.metadata)
    }

    fn best_match(&self, query_variants: &[Vec<String>]) -> Option<&MetadataEntry> {
        let mut candidates = vec![false; self.entries.len()];
        for query in query_variants {
            // Any shared token qualifies as a candidate: suffixed query ids
            // ("claude-haiku-4-5-preview") must reach entries whose first
            // token appears later in the query.
            for token in query {
                if let Some(indexes) = self.token_index.get(token) {
                    for &index in indexes {
                        candidates[index] = true;
                    }
                }
            }
        }
        self.entries
            .iter()
            .enumerate()
            .filter(|(index, _)| candidates[*index])
            .filter_map(|(_, entry)| {
                let score = query_variants
                    .iter()
                    .flat_map(|query| {
                        entry
                            .token_variants
                            .iter()
                            .filter_map(|candidate| match_score(candidate, query))
                    })
                    .max()?;
                Some((score, entry))
            })
            .min_by_key(|(score, entry)| {
                (
                    std::cmp::Reverse(*score),
                    provider_priority(&entry.key),
                    entry.key.matches('/').count(),
                    entry.key.len(),
                )
            })
            .map(|(_, entry)| entry)
    }

    fn from_entries(entries: Vec<MetadataEntry>) -> Self {
        let mut token_index = HashMap::<String, Vec<usize>>::new();
        for (index, entry) in entries.iter().enumerate() {
            let mut indexed = HashSet::new();
            for token in entry.token_variants.iter().flatten() {
                if indexed.insert(token.as_str()) {
                    token_index.entry(token.clone()).or_default().push(index);
                }
            }
        }
        Self {
            entries,
            token_index,
        }
    }
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
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join(".codex-mixin")
}

/// Codex only deserializes these input modalities; any other value (for
/// example `video` or `pdf` declared by models.dev) makes Codex reject the
/// whole generated model catalog JSON.
const CODEX_INPUT_MODALITIES: &[&str] = &["text", "image", "audio"];

fn input_modalities(input_modalities: Option<Vec<String>>, supports_vision: bool) -> Vec<String> {
    if let Some(declared) = input_modalities {
        let mut modalities: Vec<String> = Vec::new();
        for modality in declared {
            let Some(allowed) = CODEX_INPUT_MODALITIES
                .iter()
                .find(|allowed| modality.eq_ignore_ascii_case(allowed))
            else {
                continue;
            };
            if !modalities.iter().any(|existing| existing == allowed) {
                modalities.push((*allowed).to_owned());
            }
        }
        if !modalities.is_empty() {
            return modalities;
        }
    }
    if supports_vision {
        vec!["text".to_owned(), "image".to_owned()]
    } else {
        vec!["text".to_owned()]
    }
}

fn models_dev_input_modalities(model: &ModelsDevModel) -> Vec<String> {
    let declared = model
        .modalities
        .as_ref()
        .map(|modalities| modalities.input.clone())
        .unwrap_or_default();
    input_modalities(Some(declared), model_supports_image(model) == Some(true))
}

static DECIMAL_MODEL_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+)\.(\d+)").expect("valid decimal model version regex"));

const BUILTIN_MODEL_RULE_SPECS: &[(&str, u64, Option<u64>, bool)] = &[
    (r"(?i)\b(kimi[- ]?k2|kimi)\b", 262_144, Some(262_144), true),
    (r"(?i)\bminimax[- ]?m3\b", 512_000, Some(512_000), true),
    (r"(?i)\bdeepseek[- ]?v4\b", 1_000_000, Some(384_000), false),
    (r"(?i)\bglm[- ]?5[.-]?2\b", 1_048_576, Some(131_072), false),
    (r"(?i)\bglm[- ]?5\b", 200_000, Some(128_000), false),
    (r"(?i)\bglm[- ]?4[.-]?7\b", 200_000, Some(128_000), false),
    (
        r"(?i)\bclaude.*(sonnet[- ]?5|fable[- ]?5|opus[- ]?4[.-]?[678]|mythos)\b",
        1_000_000,
        Some(128_000),
        true,
    ),
    (
        r"(?i)\bclaude.*haiku[- ]?4[.-]?5\b",
        200_000,
        Some(64_000),
        true,
    ),
    (
        r"(?i)\bgpt[- ]?5[.-]?[45]\b",
        1_050_000,
        Some(128_000),
        true,
    ),
];

struct BuiltinModelRule {
    regex: Regex,
    pattern: &'static str,
    context_window: u64,
    max_output_tokens: Option<u64>,
    vision: bool,
}

static BUILTIN_MODEL_RULES: LazyLock<Vec<BuiltinModelRule>> = LazyLock::new(|| {
    BUILTIN_MODEL_RULE_SPECS
        .iter()
        .map(
            |&(pattern, context_window, max_output_tokens, vision)| BuiltinModelRule {
                regex: Regex::new(pattern).expect("valid builtin model regex"),
                pattern,
                context_window,
                max_output_tokens,
                vision,
            },
        )
        .collect()
});

fn token_variants(value: &str) -> Vec<Vec<String>> {
    let normalized = value.to_ascii_lowercase();
    let mut variants = vec![tokens(&normalized)];
    let p_variant = DECIMAL_MODEL_VERSION.replace_all(&normalized, "${1}p${2}");
    let p_tokens = tokens(&p_variant);
    if p_tokens != variants[0] {
        variants.push(p_tokens);
    }
    variants
        .into_iter()
        .filter(|variant| !variant.is_empty())
        .collect()
}

fn tokens(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

fn has_contiguous_subsequence(candidate: &[String], query: &[String]) -> bool {
    if query.is_empty() || query.len() > candidate.len() {
        return false;
    }
    candidate.windows(query.len()).any(|window| window == query)
}

/// Number of matched tokens when one id contains the other contiguously.
///
/// The score keeps "claude-haiku-4-5-preview" on the "claude-haiku-4-5" entry
/// instead of a shorter "claude-haiku-4" sibling. Reverse containment (the
/// entry inside a longer query) needs at least two tokens so single-token
/// families do not grab every id sharing one word.
fn match_score(entry_tokens: &[String], query: &[String]) -> Option<usize> {
    if has_contiguous_subsequence(entry_tokens, query) {
        return Some(query.len());
    }
    if entry_tokens.len() >= 2 && has_contiguous_subsequence(query, entry_tokens) {
        return Some(entry_tokens.len());
    }
    None
}

fn provider_priority(key: &str) -> u8 {
    // Prefer first-party catalogs over aggregators when several entries match.
    let key = key.to_ascii_lowercase();
    if key.starts_with("anthropic") || key.starts_with("zai") {
        0
    } else if key.starts_with("azure_ai/") || key.starts_with("fireworks_ai/") {
        1
    } else if key.starts_with("azure/") {
        2
    } else if key.starts_with("openrouter") {
        3
    } else if key.starts_with("bedrock/") || key.starts_with("amazon-bedrock/") {
        4
    } else {
        5
    }
}

fn builtin_metadata(model: &str, default_context_window: u64) -> ModelMetadata {
    let lower = model.to_ascii_lowercase();
    for rule in BUILTIN_MODEL_RULES.iter() {
        if rule.regex.is_match(&lower) {
            return ModelMetadata {
                context_window: rule.context_window,
                max_output_tokens: rule.max_output_tokens,
                input_modalities: if rule.vision {
                    vec!["text".to_owned(), "image".to_owned()]
                } else {
                    vec!["text".to_owned()]
                },
                supports_image: Some(rule.vision),
                supports_thinking: None,
                source: format!("builtin:{}", rule.pattern),
            };
        }
    }
    ModelMetadata {
        context_window: default_context_window,
        max_output_tokens: None,
        input_modalities: vec!["text".to_owned()],
        supports_image: None,
        supports_thinking: None,
        source: "default".to_owned(),
    }
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
    {
        return Ok(Arc::clone(&entry.catalog));
    }

    let mut stale_catalog = cached
        .as_ref()
        .filter(|entry| cache_matches(entry))
        .map(|entry| Arc::clone(&entry.catalog));
    let mut http_cache = None;
    if tokio::fs::try_exists(cache_path).await? {
        match read_models_dev_catalog(cache_path).await {
            Ok(catalog) => {
                let Some(refresh_interval) = refresh_interval else {
                    *cached = Some(CachedModelsDevCatalog {
                        catalog: Arc::clone(&catalog),
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
                                catalog: Arc::clone(&catalog),
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
    *cached = Some(CachedModelsDevCatalog {
        catalog: Arc::clone(&catalog),
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
    let fetched = match fetch_models_dev_catalog_from(client, source_url, http_cache.as_ref()).await
    {
        Ok(fetched) => fetched,
        Err(error) => {
            let Some(catalog) = stale_catalog else {
                return Err(error);
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
        crate::clients::files::write_atomic_if_changed(&cache_path, &body)
    })
    .await
    .context("models.dev cache writer task failed")??;
    Ok(())
}

/// One provider's models keyed by their models.dev ids, or None when the
/// catalog does not list the provider.
pub(crate) fn provider_models_from_catalog(
    catalog: &ModelsDevCatalog,
    provider_id: &str,
) -> Option<HashMap<String, ProviderModel>> {
    let provider = catalog.providers.get(provider_id)?;

    let mut models = HashMap::with_capacity(provider.models.len());
    for (model_id, model) in &provider.models {
        let id = model_id.trim();
        if id.is_empty() {
            continue;
        }
        models.insert(
            id.to_owned(),
            ProviderModel {
                id: id.to_owned(),
                display_name: model.name.clone(),
                description: model.description.clone(),
                context_window: model
                    .limit
                    .as_ref()
                    .and_then(|limit| limit.context.or(limit.input)),
                supports_image: model_supports_image(model),
                supports_thinking: model.reasoning,
                ..ProviderModel::default()
            },
        );
    }
    Some(models)
}

/// Fill fields the provider's own catalog left unset from models.dev data.
///
/// Provider-declared values always win; models.dev only completes the gaps.
pub(crate) fn enrich_models_with_models_dev(
    models: &mut [ProviderModel],
    metadata: &HashMap<String, ProviderModel>,
) {
    for model in models {
        if let Some(source) = metadata.get(model.id.as_str()) {
            fill_missing_model_fields(model, source);
        }
    }
}

/// Field-level precedence: keep every value the provider already declared.
pub(crate) fn fill_missing_model_fields(model: &mut ProviderModel, source: &ProviderModel) {
    if model.display_name.is_none() {
        model.display_name = source.display_name.clone();
    }
    if model.description.is_none() {
        model.description = source.description.clone();
    }
    if model.context_window.is_none() {
        model.context_window = source.context_window;
    }
    if model.supports_image.is_none() {
        model.supports_image = source.supports_image;
    }
    if model.supports_thinking.is_none() {
        model.supports_thinking = source.supports_thinking;
    }
}

/// Fill limits and capability gaps through the fuzzy models.dev resolver.
///
/// This is the fallback for providers without a direct models.dev mapping
/// (custom gateways, Baidu OneAPI, manual models): the resolver matches each
/// model id against every models.dev entry, so renamed or suffixed upstream
/// ids still find their family. Name and description stay untouched because a
/// fuzzy match must not relabel a provider's model.
pub(crate) fn fill_model_gaps_with_resolver(
    resolver: &MetadataResolver,
    models: &mut [ProviderModel],
) {
    if resolver.is_empty() {
        return;
    }
    for model in models {
        if model.context_window.is_some()
            && model.supports_image.is_some()
            && model.supports_thinking.is_some()
        {
            continue;
        }
        let matched = std::iter::once(model.id.as_str())
            .chain(model.aliases.iter().map(String::as_str))
            .find_map(|key| resolver.lookup(key));
        let Some(metadata) = matched else {
            continue;
        };
        if model.context_window.is_none() {
            model.context_window = Some(metadata.context_window);
        }
        if model.supports_image.is_none() {
            model.supports_image = metadata.supports_image;
        }
        if model.supports_thinking.is_none() {
            model.supports_thinking = metadata.supports_thinking;
        }
    }
}

fn model_supports_image(model: &ModelsDevModel) -> Option<bool> {
    match model.modalities.as_ref() {
        Some(modalities) => Some(
            modalities
                .input
                .iter()
                .any(|modality| modality.eq_ignore_ascii_case("image")),
        ),
        None if model.attachment == Some(false) => Some(false),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use axum::Router;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use serde_json::json;

    use super::*;

    #[test]
    fn resolves_limits_from_models_dev_catalog_format() {
        let resolver = MetadataResolver::from_json(&json!({
            "opencode-go": {
                "models": {
                    "glm-5.2": {
                        "name": "GLM-5.2",
                        "reasoning": true,
                        "modalities": {"input": ["text"]},
                        "limit": {"context": 1048576, "output": 131072}
                    }
                }
            },
            "anthropic": {
                "models": {
                    "claude-haiku-4-5": {
                        "attachment": true,
                        "modalities": {"input": ["text", "image"]},
                        "limit": {"context": 200000, "output": 64000}
                    }
                }
            }
        }))
        .unwrap();

        let glm = resolver.resolve("GLM-5.2", 100_000);
        assert_eq!(glm.context_window, 1_048_576);
        assert_eq!(glm.max_output_tokens, Some(131_072));
        assert_eq!(glm.input_modalities, ["text"]);
        assert_eq!(glm.source, "models.dev:opencode-go/glm-5.2");

        let haiku = resolver.resolve("claude-haiku-4-5", 100_000);
        assert_eq!(haiku.context_window, 200_000);
        assert_eq!(haiku.input_modalities, ["text", "image"]);
    }

    #[test]
    fn filters_modalities_codex_cannot_deserialize() {
        let resolver = MetadataResolver::from_json(&json!({
            "google": {
                "models": {
                    "gemini-video": {
                        "modalities": {"input": ["text", "image", "video", "pdf"]},
                        "limit": {"context": 1048576, "output": 65536}
                    },
                    "video-only": {
                        "attachment": true,
                        "modalities": {"input": ["text", "video", "audio", "pdf"]},
                        "limit": {"context": 32768, "output": 8192}
                    },
                    "attachment-only": {
                        "attachment": true,
                        "limit": {"context": 32768, "output": 8192}
                    }
                }
            }
        }))
        .unwrap();

        // Unknown modalities are dropped so Codex can parse the catalog.
        let gemini = resolver.resolve("gemini-video", 100_000);
        assert_eq!(gemini.input_modalities, ["text", "image"]);

        // Non-image attachments must not be classified as image input.
        let video_only = resolver.resolve("video-only", 100_000);
        assert_eq!(video_only.input_modalities, ["text", "audio"]);
        assert_eq!(video_only.supports_image, Some(false));

        // The generic attachment flag does not identify the attachment type.
        let attachment_only = resolver.resolve("attachment-only", 100_000);
        assert_eq!(attachment_only.input_modalities, ["text"]);
        assert_eq!(attachment_only.supports_image, None);
    }

    #[test]
    fn fuzzy_gap_fill_completes_custom_provider_models() {
        let resolver = MetadataResolver::from_json(&json!({
            "anthropic": {
                "models": {
                    "claude-haiku-4-5": {
                        "name": "Claude Haiku 4.5",
                        "attachment": true,
                        "modalities": {"input": ["text", "image"]},
                        "reasoning": true,
                        "limit": {"context": 200000, "output": 64000}
                    }
                }
            }
        }))
        .unwrap();

        let mut models = vec![
            // Renamed upstream id with a version suffix still finds its family.
            ProviderModel {
                id: "claude-haiku-4-5-preview".to_owned(),
                display_name: Some("My Haiku".to_owned()),
                ..ProviderModel::default()
            },
            // Provider-declared values always win over models.dev.
            ProviderModel {
                id: "claude-haiku-4-5".to_owned(),
                context_window: Some(123),
                supports_image: Some(false),
                ..ProviderModel::default()
            },
            // Unknown ids stay untouched.
            ProviderModel {
                id: "totally-unknown-model".to_owned(),
                ..ProviderModel::default()
            },
        ];
        fill_model_gaps_with_resolver(&resolver, &mut models);

        assert_eq!(models[0].context_window, Some(200_000));
        assert_eq!(models[0].supports_image, Some(true));
        assert_eq!(models[0].supports_thinking, Some(true));
        // A fuzzy match must not relabel the provider's model.
        assert_eq!(models[0].display_name.as_deref(), Some("My Haiku"));

        assert_eq!(models[1].context_window, Some(123));
        assert_eq!(models[1].supports_image, Some(false));
        assert_eq!(models[1].supports_thinking, Some(true));

        assert_eq!(models[2].context_window, None);
        assert_eq!(models[2].supports_image, None);
        assert_eq!(models[2].supports_thinking, None);
    }

    #[test]
    fn prefers_first_party_entries_over_aggregators() {
        let resolver = MetadataResolver::from_json(&json!({
            "openrouter": {
                "models": {
                    "claude-haiku-4-5": {"limit": {"context": 100000}}
                }
            },
            "anthropic": {
                "models": {
                    "claude-haiku-4-5": {"limit": {"context": 200000}}
                }
            }
        }))
        .unwrap();

        assert_eq!(
            resolver.resolve("claude-haiku-4-5", 1).context_window,
            200_000
        );
    }

    #[test]
    fn suffixed_ids_match_the_longest_family_entry() {
        let resolver = MetadataResolver::from_json(&json!({
            "anthropic": {
                "models": {
                    "claude-haiku-4": {"limit": {"context": 100000}},
                    "claude-haiku-4-5": {"limit": {"context": 200000}}
                }
            }
        }))
        .unwrap();

        assert_eq!(
            resolver
                .lookup("claude-haiku-4-5-20260101")
                .unwrap()
                .context_window,
            200_000
        );
        assert_eq!(
            resolver.lookup("claude-haiku-4").unwrap().context_window,
            100_000
        );
    }

    #[test]
    fn matches_decimal_and_p_version_variants() {
        let resolver = MetadataResolver::from_json(&json!({
            "fireworks-ai": {
                "models": {
                    "glm-5p2": {"limit": {"context": 1048576, "output": 131072}}
                }
            },
            "azure-ai": {
                "models": {
                    "deepseek-v4-flash": {"limit": {"context": 1000000, "output": 384000}}
                }
            }
        }))
        .unwrap();
        assert_eq!(
            resolver.resolve("GLM-5.2", 1_000_000).context_window,
            1_048_576
        );
        assert_eq!(
            resolver
                .resolve("DeepSeek-V4-Flash", 200_000)
                .max_output_tokens,
            Some(384_000)
        );
    }

    #[test]
    fn uses_builtin_family_rules_for_close_internal_aliases() {
        let resolver = MetadataResolver::empty();
        assert_eq!(
            resolver.resolve("Kimi-K2.7-Code", 1_000_000).context_window,
            262_144
        );
        assert_eq!(
            resolver.resolve("MiniMax-M3", 1_000_000).context_window,
            512_000
        );
    }

    #[test]
    fn parses_opencode_go_capabilities_from_models_dev_payload() {
        let body = r#"{
          "opencode-go": {
            "id": "opencode-go",
            "models": {
              "gpt-5.6-luna": {
                "id": "gpt-5.6-luna",
                "name": "GPT-5.6 Luna (2x usage)",
                "description": "Cost-efficient GPT-5.6 model",
                "attachment": true,
                "reasoning": true,
                "modalities": {"input": ["text", "image", "pdf"], "output": ["text"]},
                "limit": {"context": 1050000, "input": 922000, "output": 128000}
              },
              "glm-5.2": {
                "name": "GLM-5.2",
                "attachment": false,
                "reasoning": true,
                "limit": {"context": 1000000, "output": 131072}
              }
            }
          }
        }"#;

        let catalog: ModelsDevCatalog = serde_json::from_str(body).unwrap();
        let models = provider_models_from_catalog(&catalog, "opencode-go").unwrap();
        let luna = models.get("gpt-5.6-luna").unwrap();
        assert_eq!(
            luna.display_name.as_deref(),
            Some("GPT-5.6 Luna (2x usage)")
        );
        assert_eq!(luna.context_window, Some(1_050_000));
        assert_eq!(luna.supports_image, Some(true));
        assert_eq!(luna.supports_thinking, Some(true));

        let glm = models.get("glm-5.2").unwrap();
        assert_eq!(glm.context_window, Some(1_000_000));
        assert_eq!(glm.supports_image, Some(false));
        assert_eq!(glm.supports_thinking, Some(true));
    }

    #[test]
    fn enrich_fills_gaps_but_keeps_provider_declared_values() {
        let metadata = HashMap::from([(
            "gpt-5.6-luna".to_owned(),
            ProviderModel {
                id: "gpt-5.6-luna".to_owned(),
                display_name: Some("GPT-5.6 Luna (2x usage)".to_owned()),
                description: Some("from models.dev".to_owned()),
                context_window: Some(1_050_000),
                supports_image: Some(true),
                supports_thinking: Some(true),
                ..ProviderModel::default()
            },
        )]);
        let mut models = vec![ProviderModel {
            id: "gpt-5.6-luna".to_owned(),
            context_window: Some(42),
            ..ProviderModel::default()
        }];

        enrich_models_with_models_dev(&mut models, &metadata);

        assert_eq!(
            models[0].display_name.as_deref(),
            Some("GPT-5.6 Luna (2x usage)")
        );
        assert_eq!(models[0].description.as_deref(), Some("from models.dev"));
        // The provider-declared window wins over the models.dev value.
        assert_eq!(models[0].context_window, Some(42));
        assert_eq!(models[0].supports_image, Some(true));
        assert_eq!(models[0].supports_thinking, Some(true));
    }

    #[test]
    fn models_dev_refreshes_every_three_hours() {
        assert_eq!(MODELS_DEV_REFRESH_INTERVAL, Duration::from_secs(10_800));
    }

    #[tokio::test]
    async fn provider_refreshes_use_conditional_shared_cache() {
        const ETAG_VALUE: &str = "\"catalog-v1\"";
        const LAST_MODIFIED_VALUE: &str = "Mon, 14 Sep 2026 01:02:03 GMT";

        let request_count = Arc::new(AtomicUsize::new(0));
        let observed_count = Arc::clone(&request_count);
        let fail_requests = Arc::new(AtomicBool::new(false));
        let observed_fail_requests = Arc::clone(&fail_requests);
        let observed_validators = Arc::new(std::sync::Mutex::new(Vec::new()));
        let request_validators = Arc::clone(&observed_validators);
        let upstream = Router::new().route(
            "/api.json",
            get(move |headers: HeaderMap| {
                let observed_count = Arc::clone(&observed_count);
                let observed_fail_requests = Arc::clone(&observed_fail_requests);
                let request_validators = Arc::clone(&request_validators);
                async move {
                    observed_count.fetch_add(1, Ordering::Relaxed);
                    let etag = headers
                        .get(header::IF_NONE_MATCH)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    let last_modified = headers
                        .get(header::IF_MODIFIED_SINCE)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    request_validators
                        .lock()
                        .unwrap()
                        .push((etag.clone(), last_modified));
                    if observed_fail_requests.load(Ordering::Relaxed) {
                        return (StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response();
                    }
                    if etag.as_deref() == Some(ETAG_VALUE) {
                        return (StatusCode::NOT_MODIFIED, [(header::ETAG, ETAG_VALUE)])
                            .into_response();
                    }
                    (
                        [
                            (header::ETAG, ETAG_VALUE),
                            (header::LAST_MODIFIED, LAST_MODIFIED_VALUE),
                        ],
                        axum::Json(json!({
                        "deepseek": {
                            "models": {
                                "deepseek-v4": {"limit": {"context": 1000000}}
                            }
                        }
                        })),
                    )
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let cache_path = directory.path().join("models_dev_api.json");
        let source_url = format!("http://{address}/api.json");
        let client = Client::new();
        let memory_cache = tokio::sync::Mutex::new(None);

        let first = load_models_dev_catalog(
            &client,
            &source_url,
            &cache_path,
            Some(MODELS_DEV_REFRESH_INTERVAL),
            &memory_cache,
        )
        .await
        .unwrap();
        let second = load_models_dev_catalog(
            &client,
            &source_url,
            &cache_path,
            Some(MODELS_DEV_REFRESH_INTERVAL),
            &memory_cache,
        )
        .await
        .unwrap();
        let restarted_cache = tokio::sync::Mutex::new(None);
        let after_restart = load_models_dev_catalog(
            &client,
            &source_url,
            &cache_path,
            Some(MODELS_DEV_REFRESH_INTERVAL),
            &restarted_cache,
        )
        .await
        .unwrap();

        assert_eq!(request_count.load(Ordering::Relaxed), 1);
        assert!(Arc::ptr_eq(&first, &second));
        assert!(after_restart.providers.contains_key("deepseek"));
        assert!(cache_path.is_file());
        assert!(models_dev_http_cache_path(&cache_path).is_file());

        let catalog_before_304 = tokio::fs::read(&cache_path).await.unwrap();
        let expired_cache = tokio::sync::Mutex::new(None);
        let after_304 = load_models_dev_catalog(
            &client,
            &source_url,
            &cache_path,
            Some(Duration::ZERO),
            &expired_cache,
        )
        .await
        .unwrap();
        assert!(after_304.providers.contains_key("deepseek"));
        assert_eq!(
            tokio::fs::read(&cache_path).await.unwrap(),
            catalog_before_304
        );
        assert_eq!(request_count.load(Ordering::Relaxed), 2);

        let restarted_after_304 = tokio::sync::Mutex::new(None);
        load_models_dev_catalog(
            &client,
            &source_url,
            &cache_path,
            Some(MODELS_DEV_REFRESH_INTERVAL),
            &restarted_after_304,
        )
        .await
        .unwrap();
        assert_eq!(request_count.load(Ordering::Relaxed), 2);

        tokio::fs::write(&cache_path, br#"{"changed":{"models":{}}}"#)
            .await
            .unwrap();
        let mismatched_cache = tokio::sync::Mutex::new(None);
        load_models_dev_catalog(
            &client,
            &source_url,
            &cache_path,
            Some(Duration::ZERO),
            &mismatched_cache,
        )
        .await
        .unwrap();
        assert_eq!(request_count.load(Ordering::Relaxed), 3);

        fail_requests.store(true, Ordering::Relaxed);
        let failing_cache = tokio::sync::Mutex::new(None);
        load_models_dev_catalog(
            &client,
            &source_url,
            &cache_path,
            Some(Duration::ZERO),
            &failing_cache,
        )
        .await
        .unwrap();
        assert_eq!(request_count.load(Ordering::Relaxed), 4);

        let restarted_during_backoff = tokio::sync::Mutex::new(None);
        load_models_dev_catalog(
            &client,
            &source_url,
            &cache_path,
            Some(Duration::ZERO),
            &restarted_during_backoff,
        )
        .await
        .unwrap();
        assert_eq!(request_count.load(Ordering::Relaxed), 4);

        assert_eq!(
            *observed_validators.lock().unwrap(),
            vec![
                (None, None),
                (
                    Some(ETAG_VALUE.to_owned()),
                    Some(LAST_MODIFIED_VALUE.to_owned())
                ),
                (None, None),
                (
                    Some(ETAG_VALUE.to_owned()),
                    Some(LAST_MODIFIED_VALUE.to_owned())
                ),
            ]
        );
    }
}
