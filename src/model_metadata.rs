use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelMetadata {
    pub context_window: u64,
    pub max_output_tokens: Option<u64>,
    pub input_modalities: Vec<String>,
    pub source: String,
}

#[derive(Clone, Debug, Default)]
pub struct ModelMetadataResolver {
    entries: Vec<ModelMetadataEntry>,
}

#[derive(Clone, Debug)]
struct ModelMetadataEntry {
    key: String,
    token_variants: Vec<Vec<String>>,
    metadata: ModelMetadata,
}

#[derive(Debug, Deserialize)]
struct LiteLlmModelSpec {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    max_input_tokens: Option<Value>,
    #[serde(default)]
    max_tokens: Option<Value>,
    #[serde(default)]
    max_output_tokens: Option<Value>,
    #[serde(default)]
    input_modalities: Option<Vec<String>>,
    #[serde(default)]
    supports_vision: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct PiModelsJson {
    providers: BTreeMap<String, PiProvider>,
}

#[derive(Debug, Deserialize)]
struct PiProvider {
    #[serde(default)]
    models: Vec<PiModelSpec>,
}

#[derive(Debug, Deserialize)]
struct PiModelSpec {
    id: String,
    #[serde(rename = "contextWindow")]
    context_window: Option<u64>,
    #[serde(rename = "maxTokens")]
    max_tokens: Option<u64>,
    #[serde(default)]
    input: Vec<String>,
}

impl ModelMetadataResolver {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn from_json(value: &Value) -> anyhow::Result<Self> {
        if value.get("providers").is_some() {
            return Self::from_pi_json(value.clone());
        }
        let specs: BTreeMap<String, Value> = serde_json::from_value(value.clone())?;
        let mut entries = Vec::new();
        for (key, raw_spec) in specs {
            if key == "sample_spec" {
                continue;
            }
            let spec: LiteLlmModelSpec = serde_json::from_value(raw_spec)?;
            if spec.mode.as_deref().is_some_and(|mode| mode != "chat") {
                continue;
            }
            let Some(context_window) = numeric_u64(spec.max_input_tokens.as_ref())
                .or_else(|| numeric_u64(spec.max_tokens.as_ref()))
            else {
                continue;
            };
            entries.push(ModelMetadataEntry {
                token_variants: token_variants(&key),
                metadata: ModelMetadata {
                    context_window,
                    max_output_tokens: numeric_u64(spec.max_output_tokens.as_ref())
                        .or_else(|| numeric_u64(spec.max_tokens.as_ref())),
                    input_modalities: input_modalities(
                        spec.input_modalities,
                        spec.supports_vision.unwrap_or(false),
                    ),
                    source: format!("litellm:{key}"),
                },
                key,
            });
        }
        Ok(Self { entries })
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
        let cache_path = default_cache_path();
        if cache_path.exists() {
            return Self::from_json_file(&cache_path);
        }
        Ok(Self::empty())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn from_pi_json(value: Value) -> anyhow::Result<Self> {
        let parsed: PiModelsJson = serde_json::from_value(value)?;
        let mut entries = Vec::new();
        for provider in parsed.providers.values() {
            for model in &provider.models {
                let Some(context_window) = model.context_window else {
                    continue;
                };
                entries.push(ModelMetadataEntry {
                    key: model.id.clone(),
                    token_variants: token_variants(&model.id),
                    metadata: ModelMetadata {
                        context_window,
                        max_output_tokens: model.max_tokens,
                        input_modalities: if model.input.is_empty() {
                            vec!["text".to_owned()]
                        } else {
                            model.input.clone()
                        },
                        source: format!("metadata:{}", model.id),
                    },
                });
            }
        }
        Ok(Self { entries })
    }

    pub fn resolve(&self, model: &str, default_context_window: u64) -> ModelMetadata {
        let query_variants = token_variants(model);
        if let Some(entry) = self.best_litellm_match(&query_variants) {
            return entry.metadata.clone();
        }
        builtin_metadata(model, default_context_window)
    }

    fn best_litellm_match(&self, query_variants: &[Vec<String>]) -> Option<&ModelMetadataEntry> {
        self.entries
            .iter()
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
}

fn numeric_u64(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().map(|value| value as u64)),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

pub fn default_cache_path() -> PathBuf {
    if let Ok(path) = std::env::var("CODEX_GATEWAY_MODEL_METADATA_CACHE")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home)
        .join(".codex-mixin")
        .join("model_metadata_litellm.json")
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

fn token_variants(value: &str) -> Vec<Vec<String>> {
    let normalized = value.to_ascii_lowercase();
    let mut variants = vec![tokens(&normalized)];
    let decimal_as_p = Regex::new(r"(\d+)\.(\d+)").unwrap();
    let p_variant = decimal_as_p.replace_all(&normalized, "${1}p${2}");
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
    Regex::new(r"[a-z0-9]+")
        .unwrap()
        .find_iter(value)
        .map(|match_| match_.as_str().to_owned())
        .collect()
}

fn has_contiguous_subsequence(candidate: &[String], query: &[String]) -> bool {
    if query.is_empty() || query.len() > candidate.len() {
        return false;
    }
    candidate.windows(query.len()).any(|window| window == query)
}

fn provider_priority(key: &str) -> u8 {
    let key = key.to_ascii_lowercase();
    if key.starts_with("anthropic.") || key.starts_with("zai/") || key.starts_with("zai.") {
        0
    } else if key.starts_with("azure_ai/") || key.starts_with("fireworks_ai/") {
        1
    } else if key.starts_with("azure/") {
        2
    } else if key.starts_with("openrouter/") {
        3
    } else if key.starts_with("bedrock/") {
        4
    } else {
        5
    }
}

fn builtin_metadata(model: &str, default_context_window: u64) -> ModelMetadata {
    let lower = model.to_ascii_lowercase();
    for (pattern, context_window, max_output_tokens, vision) in [
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
    ] {
        if Regex::new(pattern).unwrap().is_match(&lower) {
            return ModelMetadata {
                context_window,
                max_output_tokens,
                input_modalities: if vision {
                    vec!["text".to_owned(), "image".to_owned()]
                } else {
                    vec!["text".to_owned()]
                },
                source: format!("builtin:{pattern}"),
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn matches_litellm_decimal_and_p_variants() {
        let resolver = ModelMetadataResolver::from_json(&json!({
            "fireworks_ai/glm-5p2": {"mode":"chat","max_input_tokens":1048576,"max_output_tokens":131072},
            "azure_ai/deepseek-v4-flash": {"mode":"chat","max_input_tokens":1000000,"max_output_tokens":384000}
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
        let resolver = ModelMetadataResolver::empty();
        assert_eq!(
            resolver.resolve("Kimi-K2.7-Code", 1_000_000).context_window,
            262_144
        );
        assert_eq!(
            resolver.resolve("MiniMax-M3", 1_000_000).context_window,
            512_000
        );
    }
}
