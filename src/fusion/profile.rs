use super::*;
use crate::provider::ProviderRegistry;

pub const FUSION_MODEL_PREFIX: &str = "mixin/fusion/";
pub const OFFICIAL_MODEL_PREFIX: &str = "official:";

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct PanelToolsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,
    #[serde(default = "default_max_calls_per_model")]
    pub max_calls_per_model: usize,
}

impl Default for PanelToolsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_rounds: default_max_rounds(),
            max_calls_per_model: default_max_calls_per_model(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct FusionProfile {
    pub id: String,
    pub panel_models: Vec<String>,
    pub judge_model: String,
    pub final_model: String,
    #[serde(default = "default_min_successful")]
    pub min_successful: usize,
    #[serde(default = "default_max_completion_tokens")]
    pub max_completion_tokens: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Retained for stored-config compatibility. Fusion now runs on every user turn.
    #[serde(default = "default_true")]
    pub fuse_every_user_turn: bool,
    #[serde(default = "default_true")]
    pub show_intermediate_results: bool,
    #[serde(default)]
    pub panel_tools: PanelToolsConfig,
}

impl FusionProfile {
    pub fn model_slug(&self) -> String {
        format!("{FUSION_MODEL_PREFIX}{}", self.id)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        let id = self.id.trim();
        if id.is_empty() || id.contains('/') {
            anyhow::bail!("fusion profile id must be non-empty and cannot contain '/'");
        }
        if !(1..=8).contains(&self.panel_models.len()) {
            anyhow::bail!("fusion profile {id} must configure between 1 and 8 panel models");
        }
        for model in self
            .panel_models
            .iter()
            .chain([&self.judge_model, &self.final_model])
        {
            if model.trim().is_empty() {
                anyhow::bail!("fusion profile {id} contains an empty model name");
            }
            let canonical = model.strip_prefix(OFFICIAL_MODEL_PREFIX).unwrap_or(model);
            if canonical.starts_with(FUSION_MODEL_PREFIX) {
                anyhow::bail!(
                    "fusion profile {id} cannot recursively reference fusion model {model}"
                );
            }
        }
        if self.min_successful == 0 || self.min_successful > self.panel_models.len() {
            anyhow::bail!(
                "fusion profile {id} min_successful must be between 1 and the panel model count"
            );
        }
        if self.max_completion_tokens == 0 {
            anyhow::bail!("fusion profile {id} max_completion_tokens must be greater than zero");
        }
        if self.timeout_ms == 0 {
            anyhow::bail!("fusion profile {id} timeout_ms must be greater than zero");
        }
        if self.panel_tools.enabled
            && (self.panel_tools.max_rounds == 0 || self.panel_tools.max_calls_per_model == 0)
        {
            anyhow::bail!(
                "fusion profile {id} panel tool limits must be greater than zero when tools are enabled"
            );
        }
        Ok(())
    }
}

pub fn validate_fusion_profiles(profiles: &[FusionProfile]) -> anyhow::Result<()> {
    let mut ids = HashSet::with_capacity(profiles.len());
    for profile in profiles {
        profile.validate()?;
        if !ids.insert(profile.id.as_str()) {
            anyhow::bail!("duplicate fusion profile id: {}", profile.id);
        }
    }
    Ok(())
}

pub fn validate_fusion_model_references(
    profiles: &[FusionProfile],
    providers: &ProviderRegistry,
) -> anyhow::Result<()> {
    for profile in profiles {
        for reference in profile
            .panel_models
            .iter()
            .chain([&profile.judge_model, &profile.final_model])
        {
            if let Some(official_model) = reference.strip_prefix(OFFICIAL_MODEL_PREFIX) {
                anyhow::ensure!(
                    !official_model.trim().is_empty(),
                    "fusion profile {} contains an empty official model reference",
                    profile.id
                );
                continue;
            }
            anyhow::ensure!(
                !reference.starts_with(FUSION_MODEL_PREFIX),
                "fusion profile {} cannot recursively reference fusion model {}",
                profile.id,
                reference
            );
            anyhow::ensure!(
                providers.resolve(reference).is_some(),
                "fusion profile {} references unavailable provider model {}",
                profile.id,
                reference
            );
        }
    }
    Ok(())
}

pub(super) const fn default_true() -> bool {
    true
}

pub(super) const fn default_min_successful() -> usize {
    1
}

pub(super) const fn default_max_completion_tokens() -> u64 {
    2048
}

pub(super) const fn default_timeout_ms() -> u64 {
    300_000
}

pub(super) const fn default_max_rounds() -> usize {
    16
}

pub(super) const fn default_max_calls_per_model() -> usize {
    64
}
