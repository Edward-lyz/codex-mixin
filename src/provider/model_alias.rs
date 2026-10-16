use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

pub(super) const FLAT_MODEL_PREFIX: &str = "mixin-flat-v1-";

// MiniMax subagents split provider/model on slash; encode only slash-bearing
// catalog IDs and keep the provider registry as the source of routing truth.
pub(crate) fn flat_model_id(catalog_slug: &str) -> String {
    if !catalog_slug.contains('/') {
        return catalog_slug.to_owned();
    }
    format!(
        "{FLAT_MODEL_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(catalog_slug)
    )
}

pub(super) fn unflatten_model_id(alias: &str) -> Option<String> {
    let encoded = alias.strip_prefix(FLAT_MODEL_PREFIX)?;
    let slug = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).ok()?).ok()?;
    (slug.contains('/') && flat_model_id(&slug) == alias).then_some(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_ids_are_lossless_and_canonical() {
        for slug in [
            "anthropic/claude-provider",
            "org/model/version-provider",
            "\u{6a21}\u{578b}/\u{6d4b}\u{8bd5}-provider",
        ] {
            let alias = flat_model_id(slug);
            assert!(!alias.contains('/'));
            assert_eq!(unflatten_model_id(&alias).as_deref(), Some(slug));
        }
        assert_eq!(flat_model_id("gpt-5.5-provider"), "gpt-5.5-provider");
        for invalid in [
            "mixin-flat-v1-",
            "mixin-flat-v1-%%%",
            "mixin-flat-v1-Z3B0",
            "mixin-flat-v1-YS9i=",
            "mixin-flat-v1-_w",
            "gpt-5.5",
        ] {
            assert_eq!(unflatten_model_id(invalid), None, "{invalid}");
        }
    }
}
