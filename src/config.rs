use std::env;
use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, anyhow};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::fusion::{FusionProfile, validate_fusion_model_references, validate_fusion_profiles};
use crate::provider::{CONFIG_VERSION, ProviderDefinition, ProviderRegistry};

pub use crate::provider::{
    ProviderAuthHeader as UpstreamAuthHeader, ProviderPreset, ProviderProtocol as UpstreamKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingMode {
    Off,
    Manual,
    Adaptive,
    Auto,
}

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub bind: SocketAddr,
    pub providers: Vec<ProviderDefinition>,
    pub official_responses_url: String,
    pub codex_auth_path: PathBuf,
    pub gateway_api_key: Option<String>,
    pub accept_codex_oauth: bool,
    pub default_max_tokens: u64,
    pub default_context_window: u64,
    pub request_timeout: Duration,
    pub thinking_mode: ThinkingMode,
    pub enable_web_search_tool: bool,
    pub web_search_tool_type: String,
    pub web_search_max_uses: Option<u64>,
    pub fusion_profiles: Vec<FusionProfile>,
}

impl GatewayConfig {
    pub fn from_stored_config() -> anyhow::Result<Self> {
        let stored_config = load_stored_config()?.ok_or_else(|| {
            anyhow!(
                "provider configuration is missing; run `codex-mixin providers add --preset <preset> --key <key>`"
            )
        })?;
        ensure_config_version(stored_config.config_version)?;
        if stored_config.providers.is_empty() {
            anyhow::bail!(
                "provider configuration is empty; run `codex-mixin providers add --preset <preset> --key <key>`"
            );
        }
        ProviderRegistry::new(stored_config.providers.clone())?;
        let bind = stored_config
            .gateway_bind
            .clone()
            .unwrap_or_else(|| "127.0.0.1:8787".to_owned())
            .parse()
            .context("invalid stored gateway bind")?;
        let mut fusion_profiles = stored_config.fusion_profiles.clone();
        for profile in &mut fusion_profiles {
            if profile.panel_tools.max_rounds == 4 && profile.panel_tools.max_calls_per_model == 8 {
                profile.panel_tools.max_rounds = 16;
                profile.panel_tools.max_calls_per_model = 64;
            }
        }
        let config = Self {
            bind,
            providers: stored_config.providers,
            official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
            codex_auth_path: default_codex_auth_path(),
            gateway_api_key: stored_config.gateway_api_key,
            accept_codex_oauth: true,
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_millis(600_000),
            thinking_mode: ThinkingMode::Auto,
            enable_web_search_tool: true,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            fusion_profiles,
        };
        validate_fusion_profiles(&config.fusion_profiles)?;
        Ok(config)
    }

    pub fn official_image_generation_url(&self) -> anyhow::Result<String> {
        self.official_codex_url("images/generations")
    }

    pub fn official_image_edit_url(&self) -> anyhow::Result<String> {
        self.official_codex_url("images/edits")
    }

    fn official_codex_url(&self, path: &str) -> anyhow::Result<String> {
        let base = self
            .official_responses_url
            .strip_suffix("/responses")
            .ok_or_else(|| {
                anyhow!(
                    "official responses URL must end with /responses: {}",
                    self.official_responses_url
                )
            })?;
        Ok(format!("{base}/{path}"))
    }
}

fn default_codex_auth_path() -> PathBuf {
    codex_home_path().join("auth.json")
}

fn codex_home_path() -> PathBuf {
    std::env::var("CODEX_HOME").ok().map_or_else(
        || {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".codex")
        },
        PathBuf::from,
    )
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredGatewayConfig {
    pub config_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_bind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fusion_profiles: Vec<FusionProfile>,
    pub providers: Vec<ProviderDefinition>,
}

impl Default for StoredGatewayConfig {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            gateway_bind: None,
            gateway_api_key: None,
            fusion_profiles: Vec::new(),
            providers: Vec::new(),
        }
    }
}

pub fn ensure_config_version(version: u32) -> anyhow::Result<()> {
    if version != CONFIG_VERSION {
        anyhow::bail!(
            "unsupported config version {version}; expected {CONFIG_VERSION}. Recreate the provider configuration"
        );
    }
    Ok(())
}

pub fn stored_config_path() -> PathBuf {
    if let Some(path) = env::var("CODEX_GATEWAY_CONFIG")
        .ok()
        .filter(|path| !path.is_empty())
    {
        return PathBuf::from(path);
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join(".codex-mixin").join("config.json")
}

pub fn load_stored_config() -> anyhow::Result<Option<StoredGatewayConfig>> {
    load_stored_config_from_path(&stored_config_path())
}

pub fn load_stored_config_from_path(
    path: &std::path::Path,
) -> anyhow::Result<Option<StoredGatewayConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let parsed: StoredGatewayConfig =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    ensure_config_version(parsed.config_version)?;
    Ok(Some(parsed))
}

pub fn save_stored_config(config: &StoredGatewayConfig) -> anyhow::Result<PathBuf> {
    let path = stored_config_path();
    save_stored_config_to_path(&path, config)?;
    Ok(path)
}

pub fn mutate_stored_config<T>(
    mutation: impl FnOnce(&mut StoredGatewayConfig) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    mutate_stored_config_at_path(&stored_config_path(), mutation)
}

pub fn mutate_stored_config_at_path<T>(
    path: &std::path::Path,
    mutation: impl FnOnce(&mut StoredGatewayConfig) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _lock = lock_stored_config(path)?;
    let mut config = load_stored_config_from_path(path)?.unwrap_or_default();
    let result = mutation(&mut config)?;
    save_stored_config_to_path_unlocked(path, &config)?;
    Ok(result)
}

pub fn save_stored_config_to_path(
    path: &std::path::Path,
    config: &StoredGatewayConfig,
) -> anyhow::Result<()> {
    let _lock = lock_stored_config(path)?;
    save_stored_config_to_path_unlocked(path, config)
}

fn save_stored_config_to_path_unlocked(
    path: &std::path::Path,
    config: &StoredGatewayConfig,
) -> anyhow::Result<()> {
    ensure_config_version(config.config_version)?;
    let providers = ProviderRegistry::new(config.providers.clone())?;
    validate_fusion_profiles(&config.fusion_profiles)?;
    validate_fusion_model_references(&config.fusion_profiles, &providers)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        set_private_dir_permissions(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid config filename: {}", path.display()))?;
    let temporary_path =
        path.with_file_name(format!("{file_name}.tmp.{}", uuid::Uuid::new_v4().simple()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary_path)
        .with_context(|| format!("open {}", temporary_path.display()))?;
    set_private_file_permissions(&file)?;
    let content = serde_json::to_vec_pretty(config)?;
    file.write_all(&content)
        .with_context(|| format!("write {}", temporary_path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("write {}", temporary_path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {}", temporary_path.display()))?;
    drop(file);
    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error).with_context(|| format!("replace {}", path.display()));
    }
    Ok(())
}

fn lock_stored_config(path: &std::path::Path) -> anyhow::Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        set_private_dir_permissions(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid config filename: {}", path.display()))?;
    let lock_path = path.with_file_name(format!("{file_name}.lock"));
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open {}", lock_path.display()))?;
    set_private_file_permissions(&lock)?;
    FileExt::lock_exclusive(&lock).with_context(|| format!("lock {}", lock_path.display()))?;
    Ok(lock)
}

pub fn delete_stored_config() -> anyhow::Result<bool> {
    let path = stored_config_path();
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(true)
}

fn set_private_dir_permissions(path: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 700 {}", path.display()))?;
    }
    Ok(())
}

fn set_private_file_permissions(file: &fs::File) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_loads_stored_gateway_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = StoredGatewayConfig {
            config_version: CONFIG_VERSION,
            gateway_bind: Some("127.0.0.1:18787".to_owned()),
            gateway_api_key: Some("local-key".to_owned()),
            fusion_profiles: Vec::new(),
            providers: vec![crate::provider::open_code_go_provider(
                "opencode-go",
                "opencode-key",
            )],
        };
        save_stored_config_to_path(&path, &config).unwrap();
        let loaded = load_stored_config_from_path(&path).unwrap().unwrap();
        assert_eq!(loaded.config_version, CONFIG_VERSION);
        assert_eq!(loaded.gateway_bind.as_deref(), Some("127.0.0.1:18787"));
        assert_eq!(loaded.gateway_api_key.as_deref(), Some("local-key"));
        assert_eq!(loaded.providers[0].id, "opencode-go");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn rejects_missing_or_wrong_config_version() {
        assert!(serde_json::from_str::<StoredGatewayConfig>("{}").is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(
            &path,
            r#"{"config_version":1,"providers":[],"fusion_profiles":[]}"#,
        )
        .unwrap();
        assert!(
            load_stored_config_from_path(&path)
                .unwrap_err()
                .to_string()
                .contains("unsupported config version")
        );
    }

    #[test]
    fn saves_multiple_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = StoredGatewayConfig {
            providers: vec![
                crate::provider::open_code_go_provider("primary", "one"),
                crate::provider::open_code_go_provider("backup", "two"),
            ],
            ..StoredGatewayConfig::default()
        };
        save_stored_config_to_path(&path, &config).unwrap();
        let loaded = load_stored_config_from_path(&path).unwrap().unwrap();
        assert_eq!(
            loaded
                .providers
                .iter()
                .map(|provider| provider.id.as_str())
                .collect::<Vec<_>>(),
            ["primary", "backup"]
        );
    }

    #[test]
    fn serializes_provider_mutations_with_a_config_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        save_stored_config_to_path(&path, &StoredGatewayConfig::default()).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let handles = ["first", "second"].map(|id| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                mutate_stored_config_at_path(&path, |config| {
                    config
                        .providers
                        .push(crate::provider::open_code_go_provider(id, "secret"));
                    Ok(())
                })
                .unwrap();
            })
        });
        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }
        let config = load_stored_config_from_path(&path).unwrap().unwrap();
        assert_eq!(config.providers.len(), 2);
        assert!(
            config
                .providers
                .iter()
                .any(|provider| provider.id == "first")
        );
        assert!(
            config
                .providers
                .iter()
                .any(|provider| provider.id == "second")
        );
    }

    #[test]
    fn rejects_stored_fusion_references_to_unavailable_provider_models() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = StoredGatewayConfig {
            providers: vec![crate::provider::open_code_go_provider(
                "opencode-go",
                "secret",
            )],
            fusion_profiles: vec![FusionProfile {
                id: "invalid".to_owned(),
                panel_models: vec!["missing-opencode-go".to_owned()],
                judge_model: "glm-5.2-opencode-go".to_owned(),
                final_model: "glm-5.2-opencode-go".to_owned(),
                min_successful: 1,
                max_completion_tokens: 2048,
                timeout_ms: 30_000,
                fuse_every_user_turn: true,
                show_intermediate_results: true,
                panel_tools: crate::fusion::PanelToolsConfig::default(),
            }],
            ..StoredGatewayConfig::default()
        };
        let error = save_stored_config_to_path(&path, &config).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("references unavailable provider model missing-opencode-go")
        );
        assert!(!path.exists());
    }

    #[test]
    fn resolves_official_image_generation_urls() {
        let config = GatewayConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            providers: vec![crate::provider::open_code_go_provider("opencode-go", "key")],
            official_responses_url: "https://chatgpt.example/backend-api/codex/responses"
                .to_owned(),
            codex_auth_path: PathBuf::from("/tmp/auth.json"),
            gateway_api_key: None,
            accept_codex_oauth: true,
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_secs(30),
            thinking_mode: ThinkingMode::Off,
            enable_web_search_tool: false,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            fusion_profiles: Vec::new(),
        };
        assert_eq!(
            config.official_image_generation_url().unwrap(),
            "https://chatgpt.example/backend-api/codex/images/generations"
        );
        assert_eq!(
            config.official_image_edit_url().unwrap(),
            "https://chatgpt.example/backend-api/codex/images/edits"
        );
    }
}
