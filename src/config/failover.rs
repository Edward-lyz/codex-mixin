use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{FUSION_MODEL_PREFIX, LEGACY_FUSION_MODEL_PREFIX, OFFICIAL_MODEL_PREFIX};
use crate::provider::ProviderRegistry;

pub const MAX_FAILOVER_FALLBACKS: usize = 4;

/// What a failover route last did, keyed by route entry model. The gateway writes
/// it when the answering model changes or a switch happens; the app shows it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FailoverActivity {
    /// Model that answered the latest request; `None` when every model failed.
    pub served: Option<String>,
    /// When `served` last changed.
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_switch: Option<FailoverSwitch>,
}

/// The most recent request that left the source model.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FailoverSwitch {
    pub at_ms: u64,
    /// Models that failed, in the order they were tried.
    pub failed: Vec<String>,
    pub served: Option<String>,
    /// Short cause of the source model's failure, such as `429 Too Many Requests`.
    pub reason: String,
}

fn failover_activity_path() -> PathBuf {
    super::stored_config_path().with_file_name("failover-activity.json")
}

pub(crate) fn load_failover_activity() -> anyhow::Result<BTreeMap<String, FailoverActivity>> {
    match std::fs::read(failover_activity_path()) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn save_failover_activity(
    activity: &BTreeMap<String, FailoverActivity>,
) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(activity)?;
    crate::platform::write_owner_only(&failover_activity_path(), &payload)?;
    Ok(())
}

/// Explicit failover: when `source` fails before producing output (429, 5xx,
/// connection or timeout), the gateway retries the same request against each
/// fallback in order. References use the same format as fusion profiles:
/// a provider catalog slug or `official:<model>`.
///
/// With an `id` the route is also a virtual model `auto/<id>` that clients
/// can pick by name; it enters at `source` like a direct request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FailoverRoute {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub source: String,
    pub fallbacks: Vec<String>,
}

impl FailoverRoute {
    /// The model clients request to enter this route.
    pub fn entry_model(&self) -> String {
        match &self.id {
            Some(id) => format!("{FUSION_MODEL_PREFIX}{id}"),
            None => self.source.clone(),
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_reference(&self.source, &self.source)?;
        if let Some(id) = &self.id
            && (id.trim().is_empty() || id.contains('/'))
        {
            anyhow::bail!("failover route id must be non-empty and cannot contain '/'");
        }
        if self.fallbacks.is_empty() || self.fallbacks.len() > MAX_FAILOVER_FALLBACKS {
            anyhow::bail!(
                "failover route {} must have 1 to {MAX_FAILOVER_FALLBACKS} fallback models",
                self.source
            );
        }
        let mut seen = HashSet::with_capacity(self.fallbacks.len() + 1);
        seen.insert(self.source.as_str());
        for fallback in &self.fallbacks {
            validate_reference(&self.source, fallback)?;
            if !seen.insert(fallback.as_str()) {
                anyhow::bail!(
                    "failover route {} repeats model {fallback} (fallbacks must be unique and differ from the source)",
                    self.source
                );
            }
        }
        Ok(())
    }

    pub fn models(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.source).chain(&self.fallbacks)
    }
}

fn validate_reference(source: &str, reference: &str) -> anyhow::Result<()> {
    let canonical = reference
        .strip_prefix(OFFICIAL_MODEL_PREFIX)
        .unwrap_or(reference);
    if canonical.trim().is_empty() {
        anyhow::bail!("failover route {source} contains an empty model reference");
    }
    if canonical.starts_with(FUSION_MODEL_PREFIX)
        || canonical.starts_with(LEGACY_FUSION_MODEL_PREFIX)
    {
        anyhow::bail!("failover route {source} cannot reference virtual route model {reference}");
    }
    Ok(())
}

pub fn validate_failover_routes(routes: &[FailoverRoute]) -> anyhow::Result<()> {
    let mut sources = HashSet::with_capacity(routes.len());
    let mut ids = HashSet::with_capacity(routes.len());
    for route in routes {
        route.validate()?;
        if route.id.is_none() && !sources.insert(route.source.as_str()) {
            anyhow::bail!("duplicate failover route source: {}", route.source);
        }
        if let Some(id) = &route.id
            && !ids.insert(id.as_str())
        {
            anyhow::bail!("duplicate failover route id: {id}");
        }
    }
    Ok(())
}

pub fn validate_failover_model_references(
    routes: &[FailoverRoute],
    providers: &ProviderRegistry,
) -> anyhow::Result<()> {
    for route in routes {
        for reference in route.models() {
            if reference.starts_with(OFFICIAL_MODEL_PREFIX) {
                continue;
            }
            if providers.resolve(reference).is_none() {
                anyhow::bail!(
                    "failover route {} references unavailable provider model {reference}",
                    route.source
                );
            }
        }
    }
    Ok(())
}
