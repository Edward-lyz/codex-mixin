use std::time::SystemTime;

use anyhow::{Context, ensure};
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;

use crate::provider::AwsSigV4AuthConfig;

pub(crate) fn sign_request(
    request: &mut reqwest::Request,
    auth: &AwsSigV4AuthConfig,
    payload_checksum: String,
    time: SystemTime,
) -> anyhow::Result<()> {
    let headers = request
        .headers()
        .iter()
        .map(|(name, value)| {
            Ok((
                name.as_str().to_owned(),
                value
                    .to_str()
                    .context("AWS SigV4 request contains a non-UTF-8 header")?
                    .to_owned(),
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let signable_request = SignableRequest::new(
        request.method().as_str(),
        request.url().as_str(),
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Precomputed(payload_checksum),
    )
    .context("build AWS SigV4 signable request")?;
    let credentials = Credentials::new(
        auth.access_key_id.clone(),
        auth.secret_access_key.clone(),
        auth.session_token.clone(),
        None,
        "codex-mixin",
    );
    let identity = credentials.into();
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(&auth.region)
        .name(&auth.service)
        .time(time)
        .settings(SigningSettings::default())
        .build()
        .map_err(|error| anyhow::anyhow!("build AWS SigV4 signing parameters: {error}"))?;
    let (instructions, _) = sign(signable_request, &params.into())
        .context("sign AWS Bedrock request")?
        .into_parts();
    ensure!(
        instructions.params().is_empty(),
        "AWS SigV4 unexpectedly produced query parameters"
    );
    for (name, value) in instructions.headers() {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .context("parse AWS SigV4 header name")?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .context("parse AWS SigV4 header value")?;
        request.headers_mut().insert(name, value);
    }
    Ok(())
}
