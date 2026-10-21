use crate::protocol::ResponseStream;

mod cache_shape;
mod cache_usage;
mod context_loop;
mod executor;
mod output_limit;
mod plan;
mod provider;
mod responses;
mod router;
mod turn_usage;

pub(crate) use cache_shape::{
    CacheShape, CacheShapeTracker, PrefixObservation, UpstreamCacheObserver,
    observe_upstream_cache_usage, record_provider_prefix,
};
pub(crate) use cache_usage::{DailyTokenUsage, ProviderTokenUsage, TokenUsageAggregator};
pub(crate) use context_loop::context_session;
pub(crate) use executor::GatewayExecutor;
pub(crate) use plan::{RequestPlan, UpstreamTarget};
pub(crate) use provider::{ProviderResponseRequest, stream_provider_response};
pub(crate) use responses::collect_response_stream;
pub(crate) use router::{
    AUTO_REVIEW_MODEL_SLUG, ModelRouter, ResolvedModelRoute, is_official_model_slug,
    stable_oneapi_routing,
};
pub(crate) use turn_usage::{MAX_TRACKED_TURNS, TurnQuery, TurnUsage, first_day_start_ms};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpstreamRouting {
    pub session_id: String,
    pub hash_key: String,
}
