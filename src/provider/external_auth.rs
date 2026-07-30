use std::collections::BTreeMap;

use anyhow::Context;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

pub(super) fn resolve_custom_headers_from_env(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_provider_neutral_sensitive_headers() {
        let mapping = BTreeMap::from([("x-example-auth".to_owned(), "EXAMPLE_AUTH".to_owned())]);
        let headers = resolve_custom_headers_from_env(&mapping, &|name| {
            (name == "EXAMPLE_AUTH").then(|| "signed-value".to_owned())
        })
        .unwrap();

        assert_eq!(headers["x-example-auth"], "signed-value");
        assert!(headers["x-example-auth"].is_sensitive());
    }

    #[test]
    fn missing_value_fails_without_leaking_values() {
        let mapping = BTreeMap::from([("x-example-auth".to_owned(), "MISSING_AUTH".to_owned())]);
        let error = resolve_custom_headers_from_env(&mapping, &|_| None).unwrap_err();

        assert!(error.to_string().contains("MISSING_AUTH"));
        assert!(!error.to_string().contains("signed-value"));
    }
}
