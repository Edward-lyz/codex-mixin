//! Model routes: one view over the three ways a requested model can be
//! redirected. Fusion and schedule routes are virtual models stored as fusion
//! profiles; failover routes attach to an existing provider or official model.
//! Every route is identified by the model clients request (its entry model).

use serde::{Deserialize, Serialize};

use crate::config::{
    FailoverRoute, FusionMode, FusionProfile, StoredGatewayConfig, load_stored_config,
    mutate_stored_config,
};
use crate::web_search::WebSearchCapabilities;

use super::error::OperationError;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelRoute {
    /// Virtual model that queries panel models in parallel and merges them.
    Fusion(FusionProfile),
    /// Virtual model that forwards to a different model by local time of day.
    Schedule(FusionProfile),
    /// Retry an existing model's failed request against fallback models.
    Failover(FailoverRoute),
}

impl ModelRoute {
    /// The model clients request to enter this route.
    pub fn model(&self) -> String {
        match self {
            Self::Fusion(profile) | Self::Schedule(profile) => profile.model_slug(),
            Self::Failover(route) => route.source.clone(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Fusion(_) => "fusion",
            Self::Schedule(_) => "schedule",
            Self::Failover(_) => "failover",
        }
    }

    fn from_profile(profile: FusionProfile) -> Self {
        match profile.mode {
            FusionMode::Orchestration => Self::Fusion(profile),
            FusionMode::TimeRotation => Self::Schedule(profile),
        }
    }

    /// The route kind decides the stored fusion mode, so callers never need
    /// to keep `kind` and `mode` in sync.
    fn normalized(self) -> Self {
        match self {
            Self::Fusion(mut profile) => {
                profile.mode = FusionMode::Orchestration;
                Self::Fusion(profile)
            }
            Self::Schedule(mut profile) => {
                profile.mode = FusionMode::TimeRotation;
                Self::Schedule(profile)
            }
            route @ Self::Failover(_) => route,
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Fusion(profile) | Self::Schedule(profile) => profile.validate(),
            Self::Failover(route) => route.validate(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RouteEntry {
    pub model: String,
    #[serde(flatten)]
    pub route: ModelRoute,
}

pub fn list_routes() -> anyhow::Result<Vec<RouteEntry>> {
    let stored = load_stored_config()?.unwrap_or_default();
    Ok(stored
        .fusion_profiles
        .into_iter()
        .map(ModelRoute::from_profile)
        .chain(stored.failover_routes.into_iter().map(ModelRoute::Failover))
        .map(|route| RouteEntry {
            model: route.model(),
            route,
        })
        .collect())
}

pub fn get_route(model: &str) -> anyhow::Result<Option<RouteEntry>> {
    Ok(list_routes()?
        .into_iter()
        .find(|entry| entry.model == model))
}

/// Save a route. `replace_model` names the entry model being edited, so a
/// renamed or re-kinded route replaces the old one in a single commit.
pub fn set_route(route: ModelRoute, replace_model: Option<&str>) -> Result<String, OperationError> {
    let route = route.normalized();
    route
        .validate()
        .map_err(|source| OperationError::BeforeCommit { source })?;
    let model = route.model();
    mutate_stored_config(|config| {
        upsert_route(config, route, replace_model);
        Ok(())
    })
    .map_err(|source| OperationError::BeforeCommit { source })?;
    invalidate_caches()?;
    Ok(model)
}

pub fn delete_route(model: &str) -> Result<String, OperationError> {
    mutate_stored_config(|config| {
        if remove_route(config, model) {
            Ok(())
        } else {
            anyhow::bail!("model route not found: {model}")
        }
    })
    .map_err(|source| OperationError::BeforeCommit { source })?;
    invalidate_caches()?;
    Ok(model.to_owned())
}

fn invalidate_caches() -> Result<(), OperationError> {
    WebSearchCapabilities::clear_default_cache()
        .map(|_| ())
        .map_err(|source| OperationError::AfterCommit {
            stage: "model route cache invalidation",
            source,
        })
}

fn remove_route(config: &mut StoredGatewayConfig, model: &str) -> bool {
    let before = config.fusion_profiles.len() + config.failover_routes.len();
    config
        .fusion_profiles
        .retain(|profile| profile.model_slug() != model);
    config.failover_routes.retain(|route| route.source != model);
    before != config.fusion_profiles.len() + config.failover_routes.len()
}

/// Replace in place when the edited route stays in the same collection so the
/// order of existing routes is preserved; otherwise drop the old entry and
/// append. A duplicate created by a rename is rejected by config validation.
fn upsert_route(config: &mut StoredGatewayConfig, route: ModelRoute, replace_model: Option<&str>) {
    let model = route.model();
    let previous = replace_model.unwrap_or(&model);
    match route {
        ModelRoute::Fusion(profile) | ModelRoute::Schedule(profile) => {
            config
                .failover_routes
                .retain(|route| route.source != previous);
            let profiles = &mut config.fusion_profiles;
            let index = profiles
                .iter()
                .position(|current| current.model_slug() == previous)
                .or_else(|| profiles.iter().position(|current| current.id == profile.id));
            match index {
                Some(index) => profiles[index] = profile,
                None => profiles.push(profile),
            }
        }
        ModelRoute::Failover(route) => {
            config
                .fusion_profiles
                .retain(|profile| profile.model_slug() != previous);
            let routes = &mut config.failover_routes;
            let index = routes
                .iter()
                .position(|current| current.source == previous)
                .or_else(|| {
                    routes
                        .iter()
                        .position(|current| current.source == route.source)
                });
            match index {
                Some(index) => routes[index] = route,
                None => routes.push(route),
            }
        }
    }
}
