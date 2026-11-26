//! Unified model metadata resolution.
//!
//! One resolver answers "what are this model's limits and modalities" from
//! two layered sources: the models.dev catalog (local cache, else the snapshot
//! bundled at build time), then built-in family
//! rules as the offline fallback. models.dev is also the capability source
//! for providers whose own catalog only returns model ids (OpenCode Go
//! today).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Context;
use regex::Regex;
use serde_json::Value;

use super::models_dev::{
    ModelsDevCatalog, ModelsDevModel, bundled_catalog_json, default_metadata_cache_path,
};
use super::types::ProviderModel;

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
        let raw = bundled_catalog_json()?;
        Self::from_json(&serde_json::from_slice(&raw)?)
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
                max_output_tokens: model.limit.as_ref().and_then(|limit| limit.output),
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
    if model.max_output_tokens.is_none() {
        model.max_output_tokens = source.max_output_tokens;
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
            && model.max_output_tokens.is_some()
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
        if model.max_output_tokens.is_none() {
            model.max_output_tokens = metadata.max_output_tokens;
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
