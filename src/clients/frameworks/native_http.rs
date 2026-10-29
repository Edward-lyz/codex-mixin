use anyhow::Context;
use serde_json::Value;

/// Native desktop APIs only: no proxy, redirects, external hosts or raw error
/// bodies. The dedicated runtime also permits synchronous status inspection
/// from a blocking worker without nesting the caller's Tokio runtime.
pub(super) fn request(
    method: reqwest::Method,
    url: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> anyhow::Result<Value> {
    let url = reqwest::Url::parse(url)?;
    anyhow::ensure!(
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "native application API must use an explicit loopback HTTP address"
    );
    let token = token.map(str::to_owned);
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async move {
                let timeout = if method == reqwest::Method::GET {
                    2
                } else {
                    15
                };
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(std::time::Duration::from_secs(2))
                    .timeout(std::time::Duration::from_secs(timeout))
                    .build()?;
                let mut request = client.request(method, url);
                if let Some(token) = token {
                    request = request.bearer_auth(token);
                }
                if let Some(body) = body {
                    anyhow::ensure!(
                        serde_json::to_vec(&body)?.len() <= 1_048_576,
                        "native app request is too large"
                    );
                    request = request.json(&body);
                }
                let mut response = request
                    .send()
                    .await
                    .context("native application is unavailable; open it and retry")?;
                anyhow::ensure!(
                    response.status().is_success(),
                    "native app API returned HTTP {}",
                    response.status()
                );
                anyhow::ensure!(
                    response
                        .content_length()
                        .is_none_or(|length| length <= 1_048_576),
                    "native app response is too large"
                );
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await? {
                    anyhow::ensure!(
                        bytes.len().saturating_add(chunk.len()) <= 1_048_576,
                        "native app response is too large"
                    );
                    bytes.extend_from_slice(&chunk);
                }
                if bytes.is_empty() {
                    return Ok(Value::Null);
                }
                serde_json::from_slice(&bytes).context("invalid native app API JSON")
            })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("native app HTTP worker failed"))?
}

pub(super) fn identifier(value: &str) -> anyhow::Result<&str> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
        "invalid native application identifier"
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_api_rejects_non_loopback_addresses_and_path_injection() {
        for url in [
            "https://example.com/api/providers",
            "http://localhost:23001/api/providers",
            "http://user:password@127.0.0.1:23001/api/providers",
        ] {
            assert!(request(reqwest::Method::GET, url, None, None).is_err());
        }
        for id in ["../provider", "provider/name", "", "provider?query"] {
            assert!(identifier(id).is_err());
        }
    }
}
