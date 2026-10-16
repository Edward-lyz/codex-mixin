use anyhow::ensure;

const DEFAULT_CORS_ORIGIN: &str = "*";

pub(super) fn default_cors_origins() -> Vec<String> {
    vec![DEFAULT_CORS_ORIGIN.to_owned()]
}

pub(crate) fn valid_cors_origin(origin: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    !origin.contains('*')
        && matches!(url.scheme(), "http" | "https")
        && url.origin().ascii_serialization() == origin
}

pub(crate) fn validate_cors_origins(origins: &[String]) -> anyhow::Result<()> {
    for (index, origin) in origins.iter().enumerate() {
        ensure!(
            origin == "*" || valid_cors_origin(origin),
            "cors_allowed_origins[{index}] must be '*' or a serialized HTTP(S) origin (scheme://host[:port]), not null, a partial wildcard, credentials, path, query or fragment"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cors_origins_require_canonical_http_origins() {
        for origin in [
            "http://10.252.145.171:8723",
            "https://example.com",
            "http://localhost:8723",
            "http://[::1]:8723",
        ] {
            assert!(valid_cors_origin(origin), "{origin}");
        }
        for origin in [
            "*",
            "null",
            "",
            "http://*.example.com",
            "http://example.com/",
            "http://example.com/path",
            "https://user:password@example.com",
            "http://example.com?query",
            "http://example.com#fragment",
            "http://example.com:80",
            "http://EXAMPLE.com",
            " http://example.com",
            "http://example.com\n",
            "https://example.com https://other.com",
            "file:///tmp/file",
        ] {
            assert!(!valid_cors_origin(origin), "{origin:?}");
        }
    }

    #[test]
    fn cors_defaults_migrate_to_all_origins() {
        use crate::config::{StoredGatewayConfig, migration::parse_stored_config};
        let version = crate::provider::CONFIG_VERSION;
        let parsed =
            parse_stored_config(&format!(r#"{{"config_version":{version},"providers":[]}}"#))
                .unwrap();
        assert_eq!(parsed.cors_allowed_origins, default_cors_origins());
        let legacy =
            parse_stored_config(r#"{"provider_preset":"deepseek","upstream_api_key":"fixture"}"#)
                .unwrap();
        assert_eq!(legacy.cors_allowed_origins, default_cors_origins());
        let stored = StoredGatewayConfig {
            cors_allowed_origins: Vec::new(),
            ..Default::default()
        };
        let text = serde_json::to_string(&stored).unwrap();
        assert!(text.contains("\"cors_allowed_origins\":[]"));
        assert!(
            parse_stored_config(&text)
                .unwrap()
                .cors_allowed_origins
                .is_empty()
        );
    }

    #[test]
    fn cors_invalid_config_rejected_on_load_and_save() {
        use crate::config::{
            StoredGatewayConfig, load_stored_config_from_path, save_stored_config_to_path,
        };
        for origin in ["http://*.example.com", "null", "https://example.com/path"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.json");
            let stored = StoredGatewayConfig {
                cors_allowed_origins: vec![origin.to_owned()],
                ..Default::default()
            };
            assert!(
                save_stored_config_to_path(&path, &stored)
                    .unwrap_err()
                    .to_string()
                    .contains("cors_allowed_origins[0]")
            );
            assert!(!path.exists());
            std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
            assert!(
                load_stored_config_from_path(&path)
                    .unwrap_err()
                    .to_string()
                    .contains("cors_allowed_origins[0]")
            );
            // Reuse the fixture path without deleting a computed target.
            std::fs::write(
                &path,
                serde_json::to_vec(&StoredGatewayConfig::default()).unwrap(),
            )
            .unwrap();
            // A save of invalid origins must not replace even an existing file.
            let previous = std::fs::read(&path).unwrap();
            assert!(save_stored_config_to_path(&path, &stored).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), previous);
        }
    }
}
