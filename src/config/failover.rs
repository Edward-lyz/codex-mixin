use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::{FUSION_MODEL_PREFIX, OFFICIAL_MODEL_PREFIX};
use crate::provider::ProviderRegistry;

pub const MAX_FAILOVER_FALLBACKS: usize = 4;

/// Explicit failover: when `source` fails before producing output (429, 5xx,
/// connection or timeout), the gateway retries the same request against each
/// fallback in order. References use the same format as fusion profiles:
/// a provider catalog slug or `official:<model>`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FailoverRoute {
    pub source: String,
    pub fallbacks: Vec<String>,
}

impl FailoverRoute {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_reference(&self.source, &self.source)?;
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
    if canonical.starts_with(FUSION_MODEL_PREFIX) {
        anyhow::bail!("failover route {source} cannot reference virtual route model {reference}");
    }
    Ok(())
}

pub fn validate_failover_routes(routes: &[FailoverRoute]) -> anyhow::Result<()> {
    let mut sources = HashSet::with_capacity(routes.len());
    for route in routes {
        route.validate()?;
        if !sources.insert(route.source.as_str()) {
            anyhow::bail!("duplicate failover route source: {}", route.source);
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
