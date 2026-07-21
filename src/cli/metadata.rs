use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use codex_mixin::model_metadata::{ModelMetadataResolver, default_cache_path};

pub(super) const LITELLM_MODEL_METADATA_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

pub(super) async fn refresh_metadata(output: Option<PathBuf>) -> anyhow::Result<()> {
    let output = output.unwrap_or_else(default_cache_path);
    let body = fetch_litellm_metadata().await?;
    let parsed: serde_json::Value = serde_json::from_str(&body)?;
    let resolver = ModelMetadataResolver::from_json(&parsed)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, body)?;
    println!("model metadata written: {}", output.display());
    println!("metadata entries: {}", resolver.len());
    Ok(())
}

pub(super) async fn load_model_metadata_resolver() -> anyhow::Result<ModelMetadataResolver> {
    if let Ok(path) = std::env::var("CODEX_GATEWAY_MODEL_METADATA")
        && !path.is_empty()
    {
        return ModelMetadataResolver::from_json_file(std::path::Path::new(&path));
    }
    let cache_path = default_cache_path();
    if cache_path.exists() {
        return ModelMetadataResolver::from_json_file(&cache_path);
    }
    match fetch_litellm_metadata().await {
        Ok(body) => {
            let parsed: serde_json::Value = serde_json::from_str(&body)?;
            let resolver = ModelMetadataResolver::from_json(&parsed)?;
            if let Some(parent) = cache_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&cache_path, body)?;
            eprintln!(
                "model metadata cached: {} ({} entries)",
                cache_path.display(),
                resolver.len()
            );
            Ok(resolver)
        }
        Err(err) => {
            eprintln!(
                "warning: failed to fetch LiteLLM model metadata: {err}; using built-in family rules"
            );
            Ok(ModelMetadataResolver::empty())
        }
    }
}

pub(super) async fn fetch_litellm_metadata() -> anyhow::Result<String> {
    let url = std::env::var("CODEX_GATEWAY_MODEL_METADATA_URL")
        .unwrap_or_else(|_| LITELLM_MODEL_METADATA_URL.to_owned());
    let response = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("metadata endpoint returned {status}: {body}");
    }
    Ok(body)
}
