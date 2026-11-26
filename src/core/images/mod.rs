//! Image payload handling.
//!
//! normalize: recompress and canonicalize inline images in request bodies
//! so replayed prompts stay byte-identical for prompt caching, with an
//! aggressive fallback profile for 413 retries.
//! generation: the marker-based registry that routes imagegen tool calls
//! back to the auxiliary provider that should serve them.

mod generation;
mod normalize;

pub(crate) use generation::ImageRouteRegistry;
pub(crate) use normalize::{
    ImageCompressionProfile, normalize_anthropic_images_blocking,
    normalize_provider_images_blocking, normalize_provider_images_for_fallback,
};
