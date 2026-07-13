use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

const ROUTE_MARKER_START: &str = "\n\n<!-- codex-mixin-image-route:";
const ROUTE_MARKER_END: &str = " -->";
const ROUTE_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Default)]
pub(crate) struct ImageRouteRegistry {
    routes: Arc<Mutex<HashMap<String, Instant>>>,
}

impl ImageRouteRegistry {
    pub(crate) fn mark_arguments(&self, arguments: &str) -> Result<String, String> {
        let mut arguments: Value = serde_json::from_str(arguments)
            .map_err(|error| format!("imagegen arguments are not valid JSON: {error}"))?;
        let prompt = arguments
            .get("prompt")
            .and_then(Value::as_str)
            .filter(|prompt| !prompt.trim().is_empty())
            .ok_or_else(|| "imagegen arguments must contain a non-empty prompt".to_owned())?;
        match arguments.get("referenced_image_paths") {
            None | Some(Value::Null) => {}
            Some(Value::Array(paths)) if paths.is_empty() => {}
            Some(Value::Array(_)) => {
                return Err(
                    "the configured upstream supports image generation but not Codex image editing"
                        .to_owned(),
                );
            }
            Some(_) => {
                return Err("imagegen referenced_image_paths must be an array".to_owned());
            }
        }
        match arguments.get("num_last_images_to_include") {
            None | Some(Value::Null) => {}
            Some(count) if count.as_u64() == Some(0) => {}
            Some(count) if count.as_u64().is_some() => {
                return Err(
                    "the configured upstream supports image generation but not Codex image editing"
                        .to_owned(),
                );
            }
            Some(_) => {
                return Err(
                    "imagegen num_last_images_to_include must be a non-negative integer".to_owned(),
                );
            }
        }

        let route_id = Uuid::new_v4().simple().to_string();
        arguments["prompt"] = Value::String(format!(
            "{prompt}{ROUTE_MARKER_START}{route_id}{ROUTE_MARKER_END}"
        ));
        let encoded = serde_json::to_string(&arguments)
            .map_err(|error| format!("encode routed imagegen arguments: {error}"))?;
        let now = Instant::now();
        let mut routes = self
            .routes
            .lock()
            .map_err(|_| "image route registry lock poisoned".to_owned())?;
        routes.retain(|_, expires_at| *expires_at > now);
        routes.insert(route_id, now + ROUTE_TTL);
        Ok(encoded)
    }

    pub(crate) fn resolve_prompt(&self, prompt: &str) -> Result<Option<String>, String> {
        let Some((clean_prompt, marker)) = prompt.rsplit_once(ROUTE_MARKER_START) else {
            if prompt.contains("codex-mixin-image-route:") {
                return Err("malformed codex-mixin image route marker".to_owned());
            }
            return Ok(None);
        };
        let route_id = marker
            .strip_suffix(ROUTE_MARKER_END)
            .filter(|route_id| {
                route_id.len() == 32 && route_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or_else(|| "malformed codex-mixin image route marker".to_owned())?;
        let now = Instant::now();
        let mut routes = self
            .routes
            .lock()
            .map_err(|_| "image route registry lock poisoned".to_owned())?;
        routes.retain(|_, expires_at| *expires_at > now);
        if !routes.contains_key(route_id) {
            return Err("unknown or expired codex-mixin image route marker".to_owned());
        }
        Ok(Some(clean_prompt.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_and_resolves_generation_without_exposing_marker_upstream() {
        let registry = ImageRouteRegistry::default();
        let marked = registry
            .mark_arguments(
                r#"{"prompt":"draw a square","referenced_image_paths":[],"num_last_images_to_include":0}"#,
            )
            .unwrap();
        let marked: Value = serde_json::from_str(&marked).unwrap();
        let marked_prompt = marked["prompt"].as_str().unwrap();
        assert!(marked_prompt.contains("codex-mixin-image-route:"));
        assert_eq!(
            registry.resolve_prompt(marked_prompt).unwrap().as_deref(),
            Some("draw a square")
        );
        assert!(registry.resolve_prompt("draw a square").unwrap().is_none());
    }

    #[test]
    fn rejects_unknown_markers_and_image_edits() {
        let registry = ImageRouteRegistry::default();
        let unknown = "draw\n\n<!-- codex-mixin-image-route:00000000000000000000000000000000 -->";
        assert!(
            registry
                .resolve_prompt(unknown)
                .unwrap_err()
                .contains("expired")
        );
        assert!(
            registry
                .mark_arguments(r#"{"prompt":"edit","referenced_image_paths":["/tmp/a.png"]}"#)
                .unwrap_err()
                .contains("not Codex image editing")
        );
    }

    #[test]
    fn resolves_the_appended_marker_when_prompt_contains_marker_text() {
        let registry = ImageRouteRegistry::default();
        let prompt = "explain\n\n<!-- codex-mixin-image-route: from the source code";
        let marked = registry
            .mark_arguments(&serde_json::json!({"prompt":prompt}).to_string())
            .unwrap();
        let marked: Value = serde_json::from_str(&marked).unwrap();

        assert_eq!(
            registry
                .resolve_prompt(marked["prompt"].as_str().unwrap())
                .unwrap()
                .as_deref(),
            Some(prompt)
        );
    }
}
