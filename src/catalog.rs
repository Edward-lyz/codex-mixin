pub(super) const FALLBACK_BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent. Work in the user's workspace, use tools carefully, and keep responses concise.";
pub(super) const CUSTOM_MODEL_MARKER: &str = "codex_mixin_managed";
pub(super) const UPSTREAM_MODEL_MARKER: &str = "codex_mixin_upstream_model";
pub(super) const SUPPORTS_THINKING_MARKER: &str = "codex_mixin_supports_thinking";

mod generation;
mod managed;
pub mod official;
mod service;
mod template;

pub use generation::{codex_catalog_from_models_with_metadata, codex_oauth_proxy_catalog};
pub use managed::{
    apply_official_context_overrides, migrate_managed_model_metadata, refresh_managed_oauth_catalog,
};
pub(crate) use service::CatalogService;
pub use template::{
    apply_auto_review_override, apply_web_search_capabilities, load_template_catalog,
};
