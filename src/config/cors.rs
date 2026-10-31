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
