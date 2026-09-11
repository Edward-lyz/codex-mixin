use super::*;
use crate::provider::ProviderRegistry;
use chrono::{Local, Timelike};

pub const FUSION_MODEL_PREFIX: &str = "mixin/fusion/";
pub const OFFICIAL_MODEL_PREFIX: &str = "official:";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FusionMode {
    #[default]
    Orchestration,
    TimeRotation,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct FusionTimeRoute {
    pub start_minute: u16,
    pub end_minute: u16,
    pub model: String,
}

impl FusionTimeRoute {
    fn contains(&self, minute: u16) -> bool {
        if self.start_minute < self.end_minute {
            (self.start_minute..self.end_minute).contains(&minute)
        } else {
            minute >= self.start_minute || minute < self.end_minute
        }
    }
}

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
    #[serde(default)]
    pub mode: FusionMode,
    #[serde(default)]
    pub time_routes: Vec<FusionTimeRoute>,
    pub panel_models: Vec<String>,
    pub judge_model: String,
    pub final_model: String,
    #[serde(default = "default_min_successful")]
    pub min_successful: usize,
    #[serde(default = "default_max_completion_tokens")]
    pub max_completion_tokens: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
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
        if self.max_completion_tokens == 0 {
            anyhow::bail!("fusion profile {id} max_completion_tokens must be greater than zero");
        }
        if self.timeout_ms == 0 {
            anyhow::bail!("fusion profile {id} timeout_ms must be greater than zero");
        }
        match self.mode {
            FusionMode::Orchestration => self.validate_orchestration(id),
            FusionMode::TimeRotation => self.validate_time_rotation(id),
        }
    }

    pub fn active_model_at_minute(&self, minute: u16) -> &str {
        if self.mode == FusionMode::TimeRotation {
            return self
                .time_routes
                .iter()
                .find(|route| route.contains(minute))
                .map_or(self.final_model.as_str(), |route| route.model.as_str());
        }
        &self.final_model
    }

    pub fn active_model_now(&self) -> &str {
        let now = Local::now();
        self.active_model_at_minute((now.hour() * 60 + now.minute()) as u16)
    }

    fn validate_orchestration(&self, id: &str) -> anyhow::Result<()> {
        if !(1..=8).contains(&self.panel_models.len()) {
            anyhow::bail!("fusion profile {id} must configure between 1 and 8 panel models");
        }
        for model in self
            .panel_models
            .iter()
            .chain([&self.judge_model, &self.final_model])
        {
            validate_model_reference(id, model)?;
        }
        if self.min_successful == 0 || self.min_successful > self.panel_models.len() {
            anyhow::bail!(
                "fusion profile {id} min_successful must be between 1 and the panel model count"
            );
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

    fn validate_time_rotation(&self, id: &str) -> anyhow::Result<()> {
        validate_model_reference(id, &self.final_model)?;
        if self.time_routes.is_empty() || self.time_routes.len() > 24 {
            anyhow::bail!("fusion profile {id} must configure between 1 and 24 time routes");
        }
        let mut occupied = [false; 1_440];
        for route in &self.time_routes {
            if route.start_minute >= 1_440
                || route.end_minute >= 1_440
                || route.start_minute == route.end_minute
            {
                anyhow::bail!("fusion profile {id} contains an invalid time range");
            }
            validate_model_reference(id, &route.model)?;
            for (minute, is_occupied) in occupied.iter_mut().enumerate() {
                if route.contains(minute as u16) && std::mem::replace(is_occupied, true) {
                    anyhow::bail!("fusion profile {id} contains overlapping time routes");
                }
            }
        }
        Ok(())
    }
}

fn validate_model_reference(profile_id: &str, model: &str) -> anyhow::Result<()> {
    if model.trim().is_empty() {
        anyhow::bail!("fusion profile {profile_id} contains an empty model name");
    }
    let canonical = model.strip_prefix(OFFICIAL_MODEL_PREFIX).unwrap_or(model);
    if canonical.starts_with(FUSION_MODEL_PREFIX) {
        anyhow::bail!(
            "fusion profile {profile_id} cannot recursively reference fusion model {model}"
        );
    }
    Ok(())
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
        let references = match profile.mode {
            FusionMode::Orchestration => profile
                .panel_models
                .iter()
                .chain([&profile.judge_model, &profile.final_model])
                .collect::<Vec<_>>(),
            FusionMode::TimeRotation => profile
                .time_routes
                .iter()
                .map(|route| &route.model)
                .chain([&profile.final_model])
                .collect::<Vec<_>>(),
        };
        for reference in references {
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
