use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use codex_mixin::provider::{MODELS_DEV_API_URL, MetadataResolver, default_metadata_cache_path};

const METADATA_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) async fn refresh_metadata(output: Option<PathBuf>) -> anyhow::Result<()> {
    let output = output.unwrap_or_else(default_metadata_cache_path);
    let body = fetch_metadata_catalog().await?;
    let parsed: serde_json::Value = serde_json::from_str(&body)?;
    let resolver = MetadataResolver::from_json(&parsed)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, body)?;
    println!("model metadata written: {}", output.display());
    println!("metadata entries: {}", resolver.len());
    Ok(())
}

pub(super) async fn load_model_metadata_resolver() -> anyhow::Result<MetadataResolver> {
    let cached = MetadataResolver::from_default_files()?;
    if !cached.is_empty() {
        return Ok(cached);
    }
    match fetch_metadata_catalog().await {
        Ok(body) => {
            let parsed: serde_json::Value = serde_json::from_str(&body)?;
            let resolver = MetadataResolver::from_json(&parsed)?;
            let cache_path = default_metadata_cache_path();
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
                "warning: failed to fetch the models.dev catalog: {err}; using built-in family rules"
            );
            Ok(MetadataResolver::empty())
        }
    }
}

async fn fetch_metadata_catalog() -> anyhow::Result<String> {
    let url = std::env::var("CODEX_GATEWAY_MODEL_METADATA_URL")
        .unwrap_or_else(|_| MODELS_DEV_API_URL.to_owned());
    let response = reqwest::Client::new()
        .get(&url)
        .timeout(METADATA_FETCH_TIMEOUT)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("metadata endpoint returned {status}: {body}");
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_metadata_cannot_block_an_interactive_install() {
        assert!(METADATA_FETCH_TIMEOUT <= Duration::from_secs(5));
    }
}
