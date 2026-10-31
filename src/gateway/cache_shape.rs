//! Prompt-prefix cache contract for provider-visible requests.
//!
//! Providers cache on the rendered prompt prefix: system prompt, tool
//! definitions, then the message sequence in order. A session keeps that cache
//! only while those regions stay byte-identical and every new turn is appended
//! at the tail. This module derives the shape from the exact bytes each
//! protocol serializes upstream and reports where a session lost its prefix, so
//! a cache miss has a concrete cause instead of a guess.

mod shape;
mod tracking;

pub(crate) use shape::CacheShape;
pub(crate) use tracking::{
    CacheShapeTracker, PrefixObservation, UpstreamCacheObserver, observe_upstream_cache_usage,
    record_provider_prefix,
};
