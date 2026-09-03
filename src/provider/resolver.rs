//! Unified model metadata resolution.
//!
//! One resolver answers "what are this model's limits and modalities" from
//! two layered sources: a cached models.dev catalog, then built-in family
//! rules as the offline fallback. models.dev is also the capability source
//! for providers whose own catalog only returns model ids (OpenCode Go
//! today).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, anyhow};
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use super::types::ProviderModel;

pub const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelMetadata {
    pub context_window: u64,
    pub max_output_tokens: Option<u64>,
    pub input_modalities: Vec<String>,
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
struct ModelsDevCatalog {
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
                        source: format!("models.dev:{key}"),
                    },
                    key,
                });
            }
        }
        Ok(Self::from_entries(entries))
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

    fn best_match(&self, query_variants: &[Vec<String>]) -> Option<&MetadataEntry> {
        let mut candidates = vec![false; self.entries.len()];
        for query in query_variants {
            let Some(first_token) = query.first() else {
                continue;
            };
            if let Some(indexes) = self.token_index.get(first_token) {
                for &index in indexes {
                    candidates[index] = true;
                }
            }
        }
        self.entries
            .iter()
            .enumerate()
            .filter(|(index, _)| candidates[*index])
            .map(|(_, entry)| entry)
            .filter(|entry| {
                query_variants.iter().any(|query| {
                    entry
                        .token_variants
                        .iter()
                        .any(|candidate| has_contiguous_subsequence(candidate, query))
                })
            })
            .min_by_key(|entry| {
                (
                    provider_priority(&entry.key),
                    entry.key.matches('/').count(),
                    entry.key.len(),
                )
            })
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

fn input_modalities(input_modalities: Option<Vec<String>>, supports_vision: bool) -> Vec<String> {
    if let Some(modalities) = input_modalities.filter(|modalities| !modalities.is_empty()) {
        return modalities;
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
    input_modalities(Some(declared), model.attachment == Some(true))
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
                source: format!("builtin:{}", rule.pattern),
            };
        }
    }
    ModelMetadata {
        context_window: default_context_window,
        max_output_tokens: None,
        input_modalities: vec!["text".to_owned()],
        source: "default".to_owned(),
    }
}

/// Fetch one provider's model capability metadata from models.dev.
///
/// Some provider catalogs return IDs only (OpenCode Go, DeepSeek) or omit
/// capabilities entirely (AWS Bedrock control plane); models.dev supplies the
/// missing limits and capability flags for those.
pub(crate) async fn fetch_models_dev_provider_models(
    client: &Client,
    provider_id: &str,
) -> anyhow::Result<HashMap<String, ProviderModel>> {
    let response = client
        .get(MODELS_DEV_API_URL)
        .header(reqwest::header::USER_AGENT, "codex-mixin")
        .send()
        .await
        .with_context(|| format!("failed to request models.dev catalog for {provider_id}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read models.dev catalog body")?;
    if !status.is_success() {
        return Err(anyhow!(
            "models.dev catalog returned {status} for {provider_id}: {body}"
        ));
    }
    parse_models_dev_provider_models(provider_id, &body)
}

pub(crate) fn parse_models_dev_provider_models(
    provider_id: &str,
    body: &str,
) -> anyhow::Result<HashMap<String, ProviderModel>> {
    let catalog: ModelsDevCatalog =
        serde_json::from_str(body).context("models.dev catalog returned invalid JSON")?;
    let Some(provider) = catalog.providers.get(provider_id) else {
        return Err(anyhow!(
            "models.dev catalog does not include provider {provider_id}"
        ));
    };

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
                supports_image: Some(model_supports_image(model)),
                supports_thinking: model.reasoning,
                ..ProviderModel::default()
            },
        );
    }
    Ok(models)
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

fn model_supports_image(model: &ModelsDevModel) -> bool {
    if model.attachment == Some(true) {
        return true;
    }
    model.modalities.as_ref().is_some_and(|modalities| {
        modalities
            .input
            .iter()
            .any(|modality| modality.eq_ignore_ascii_case("image"))
    })
}

#[cfg(test)]
mod tests {
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

        let models = parse_models_dev_provider_models("opencode-go", body).unwrap();
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
}
