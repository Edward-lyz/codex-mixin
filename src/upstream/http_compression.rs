use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};
use serde::Serialize;

use super::UpstreamAccess;
use super::body::{JsonFile, prepare_json_body};
use crate::error::GatewayError;
use crate::provider::AwsSigV4AuthConfig;

impl UpstreamAccess {
    pub(crate) async fn send_json<T>(
        &self,
        request: reqwest::RequestBuilder,
        value: T,
        aws: Option<&AwsSigV4AuthConfig>,
    ) -> Result<reqwest::Response, GatewayError>
    where
        T: Serialize + Send + 'static,
    {
        let (client, base) = request.build_split();
        let base = base?;
        let mut endpoint = base.url().clone();
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        let support = self.gzip_support.lock().await.get(&endpoint).copied();
        let compress = support != Some(false);
        let prepared = prepare_json_body(value, compress, aws.is_some()).await?;
        let Some(gzip) = prepared.gzip else {
            return self
                .send_json_file(&client, base, prepared.raw, false, aws)
                .await;
        };
        if support.is_none() {
            let mut probe = base.try_clone().ok_or_else(|| {
                GatewayError::Other(anyhow::anyhow!("cannot negotiate JSON request encoding"))
            })?;
            *probe.method_mut() = reqwest::Method::OPTIONS;
            probe.headers_mut().remove(CONTENT_LENGTH);
            probe.headers_mut().remove(CONTENT_ENCODING);
            let response = self
                .send_official(reqwest::RequestBuilder::from_parts(client.clone(), probe))
                .await?;
            let accepts_gzip = response
                .headers()
                .get(ACCEPT_ENCODING)
                .and_then(|header| header.to_str().ok())
                .is_some_and(accepts_gzip);
            self.gzip_support
                .lock()
                .await
                .insert(endpoint.clone(), accepts_gzip);
            if !accepts_gzip {
                tracing::debug!(
                    "upstream did not advertise gzip request support; sending original JSON"
                );
                return self
                    .send_json_file(&client, base, prepared.raw, false, aws)
                    .await;
            }
        }
        let retry = base.try_clone().ok_or_else(|| {
            GatewayError::Other(anyhow::anyhow!(
                "cannot replay JSON request after encoding rejection"
            ))
        })?;
        let response = self.send_json_file(&client, base, gzip, true, aws).await?;
        if response.status() != reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE {
            return Ok(response);
        }
        // A 415 rejects the representation before execution. Never replay on
        // connection errors, timeouts, 413, or other application failures.
        tracing::warn!(
            status = 415,
            request_encoding = "gzip",
            "upstream rejected compressed JSON; retrying the original representation once"
        );
        drop(response);
        let response = self
            .send_json_file(&client, retry, prepared.raw, false, aws)
            .await?;
        if response.status().is_success() {
            self.gzip_support.lock().await.insert(endpoint, false);
        }
        Ok(response)
    }

    async fn send_json_file(
        &self,
        client: &reqwest::Client,
        base: reqwest::Request,
        body: JsonFile,
        gzip: bool,
        aws: Option<&AwsSigV4AuthConfig>,
    ) -> Result<reqwest::Response, GatewayError> {
        let mut request = reqwest::RequestBuilder::from_parts(client.clone(), base)
            .body(body.file)
            .build()?;
        // These headers describe the bytes produced here, not a client or
        // provider's custom headers from before protocol conversion.
        request.headers_mut().remove(CONTENT_ENCODING);
        request.headers_mut().insert(
            CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        request.headers_mut().insert(
            CONTENT_LENGTH,
            reqwest::header::HeaderValue::from_str(&body.length.to_string())
                .map_err(|error| GatewayError::Other(error.into()))?,
        );
        if gzip {
            request.headers_mut().insert(
                CONTENT_ENCODING,
                reqwest::header::HeaderValue::from_static("gzip"),
            );
        }
        if let Some(aws) = aws {
            crate::provider::sign_aws_request(
                &mut request,
                aws,
                body.sha256.ok_or_else(|| {
                    GatewayError::Other(anyhow::anyhow!("signed JSON payload digest is missing"))
                })?,
                std::time::SystemTime::now(),
            )?;
        }
        self.send_official(reqwest::RequestBuilder::from_parts(client.clone(), request))
            .await
    }
}

fn accepts_gzip(header: &str) -> bool {
    header.split(',').any(|encoding| {
        let mut parts = encoding.trim().split(';');
        if !matches!(parts.next().map(str::trim), Some("gzip" | "*")) {
            return false;
        }
        parts.all(|parameter| {
            parameter
                .trim()
                .strip_prefix("q=")
                .and_then(|quality| quality.parse::<f32>().ok())
                .is_some_and(|quality| quality > 0.0 && quality <= 1.0)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn honors_request_encoding_quality() {
        assert!(accepts_gzip("zstd, gzip"));
        assert!(accepts_gzip("gzip;q=0.5"));
        assert!(!accepts_gzip("gzip;q=0"));
        assert!(!accepts_gzip("gzip;q=invalid"));
        assert!(!accepts_gzip("br, zstd"));
    }
}
