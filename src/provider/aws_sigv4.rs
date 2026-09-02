use std::time::SystemTime;

use anyhow::{Context, ensure};
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;

use super::AwsSigV4AuthConfig;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_bedrock_request_with_session_credentials() {
        let client = reqwest::Client::new();
        let mut request = client
            .post("https://bedrock-mantle.us-east-1.api.aws/anthropic/v1/messages")
            .header("content-type", "application/json")
            .body("{}")
            .build()
            .unwrap();
        let auth = AwsSigV4AuthConfig {
            access_key_id: "AKIDEXAMPLE".to_owned(),
            secret_access_key: "secret-example".to_owned(),
            session_token: Some("session-example".to_owned()),
            region: "us-east-1".to_owned(),
            service: "bedrock-mantle".to_owned(),
        };

        sign_request(
            &mut request,
            &auth,
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a".to_owned(),
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
        )
        .unwrap();

        let authorization = request.headers()["authorization"].to_str().unwrap();
        assert!(authorization.contains("Credential=AKIDEXAMPLE/"));
        assert!(!authorization.contains("secret-example"));
        assert_eq!(request.headers()["x-amz-security-token"], "session-example");
    }
}
