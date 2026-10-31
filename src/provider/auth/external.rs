use std::collections::BTreeMap;

use anyhow::Context;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

pub(crate) fn resolve_custom_headers_from_env(
    mapping: &BTreeMap<String, String>,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, variable) in mapping {
        let name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid custom header name {name}"))?;
        let value = lookup(variable)
            .filter(|value| !value.is_empty())
            .with_context(|| {
                format!("custom header {name} requires non-empty environment variable {variable}")
            })?;
        let mut value = HeaderValue::from_str(&value).with_context(|| {
            format!("environment variable {variable} has invalid value for {name}")
        })?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    Ok(headers)
}
