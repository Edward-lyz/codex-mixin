//! Gemini Enterprise license discovery and regional request preparation.
use anyhow::{Context, ensure};
use reqwest::header::{HeaderMap, USER_AGENT};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ProviderModel, ProviderRuntime};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct License {
    project_id: String,
    location: String,
    user_tier: String,
}

fn validate_target(license: &License) -> anyhow::Result<()> {
    ensure!(
        !license.project_id.is_empty()
            && license.project_id.len() <= 128
            && license
                .project_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid AGY Business project ID"
    );
    ensure!(
        matches!(license.location.as_str(), "global" | "us" | "eu"),
        "AGY Business location must be global, us or eu"
    );
    ensure!(
        !license.user_tier.trim().is_empty(),
        "AGY Business license has no userTier"
    );
    Ok(())
}

fn validate_provider(provider: &ProviderRuntime) -> anyhow::Result<()> {
    let url = provider.api_url();
    ensure!(
        url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && url.host_str() == Some("businessaicode.googleapis.com")
            && url.path() == "/v1beta"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "AGY Business native credentials require https://businessaicode.googleapis.com/v1beta"
    );
    Ok(())
}

async fn auth_headers(provider: &ProviderRuntime, force: bool) -> anyhow::Result<HeaderMap> {
    validate_provider(provider)?;
    let mut headers = if provider.definition().auth.api_key.trim().is_empty() {
        super::auth::agy_business::headers(
            force,
            &crate::platform::oauth::oauth_path("agy-business"),
            &crate::platform::oauth::agy_external_token_paths(),
            &crate::platform::oauth::google_adc_path(),
            "https://oauth2.googleapis.com/token",
        )
        .await?
    } else {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", provider.definition().auth.api_key).parse()?,
        );
        headers
    };
    headers.insert(
        USER_AGENT,
        super::auth::agy_business::USER_AGENT_VALUE.parse()?,
    );
    Ok(headers)
}

async fn license(client: &reqwest::Client, headers: &HeaderMap) -> anyhow::Result<License> {
    let settings =
        super::auth::agy_business::read_optional(&crate::platform::oauth::agy_settings_path())
            .await?;
    let adc = super::auth::agy_business::read_optional(&crate::platform::oauth::google_adc_path())
        .await?;
    let project = settings
        .as_ref()
        .and_then(|settings| settings.pointer("/gcp/project"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            adc.as_ref()
                .and_then(|adc| adc.get("quota_project_id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| std::env::var("GOOGLE_CLOUD_QUOTA_PROJECT").ok());
    let location = settings
        .as_ref()
        .and_then(|settings| settings.pointer("/gcp/location"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| std::env::var("GOOGLE_CLOUD_LOCATION").ok());
    let mut request = client
        .get("https://businessaicode.googleapis.com/v1beta:fetchLicenses")
        .headers(headers.clone())
        .timeout(std::time::Duration::from_secs(20));
    if let Some(project) = &project {
        ensure!(
            !project.is_empty()
                && project
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "invalid AGY Business configured project"
        );
        request = request.header("x-goog-user-project", project);
    }
    let response = request.send().await?;
    ensure!(
        response.status().is_success(),
        "AGY Business license discovery returned {}; check project access and Gemini Enterprise provisioning",
        response.status()
    );
    let report = super::agy::bounded_json(response, "Business licenses").await?;
    let licenses = report
        .get("licenses")
        .context("Business license response has no licenses")?;
    let mut licenses: Vec<License> = serde_json::from_value(licenses.clone())?;
    licenses.retain(|license| {
        project
            .as_ref()
            .is_none_or(|project| &license.project_id == project)
            && location
                .as_ref()
                .is_none_or(|location| &license.location == location)
    });
    ensure!(
        licenses.len() == 1,
        "AGY Business requires one selected license, found {}; select gcp.project and gcp.location in agy settings",
        licenses.len()
    );
    let license = licenses.pop().context("AGY Business license is missing")?;
    validate_target(&license)?;
    Ok(license)
}

pub(crate) async fn discover_models(
    client: &reqwest::Client,
    provider: &ProviderRuntime,
) -> anyhow::Result<Vec<ProviderModel>> {
    let headers = auth_headers(provider, false).await?;
    let license = license(client, &headers).await?;
    let response = client.post("https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels")
        .headers(headers).timeout(std::time::Duration::from_secs(20))
        .json(&json!({"project":license.project_id,"location":license.location,"entitlement":{"userTier":license.user_tier}})).send().await?;
    ensure!(
        response.status().is_success(),
        "AGY Business model directory returned {}; consumer credentials must not be used for Business discovery",
        response.status()
    );
    super::agy::parse_models(&super::agy::bounded_json(response, "Business models").await?)
}

pub(crate) async fn prepare_request(
    client: &reqwest::Client,
    provider: &ProviderRuntime,
    model: &str,
    body: Value,
    force: bool,
) -> anyhow::Result<(reqwest::Url, HeaderMap, Value)> {
    let mut headers = auth_headers(provider, force).await?;
    let license = license(client, &headers).await?;
    let (url, body) = inference_request(&license, model, body)?;
    let trajectory = uuid::Uuid::new_v4().to_string();
    headers.insert("x-goog-user-project", license.project_id.parse()?);
    headers.insert("x-aicode-trajectory-id", trajectory.parse()?);
    headers.insert("x-aicode-request-id", format!("{trajectory}-0").parse()?);
    Ok((url, headers, body))
}

fn inference_request(
    license: &License,
    model: &str,
    mut body: Value,
) -> anyhow::Result<(reqwest::Url, Value)> {
    validate_target(license)?;
    let host = if license.location == "global" {
        "businessaicode.googleapis.com".to_owned()
    } else {
        format!("businessaicode.{}.rep.googleapis.com", license.location)
    };
    let url = reqwest::Url::parse(&format!(
        "https://{host}/v1beta/projects/{}/locations/{}:streamGenerateContent?alt=sse",
        license.project_id, license.location
    ))?;
    let object = body
        .as_object_mut()
        .context("Business request must be an object")?;
    object.retain(|key, _| {
        matches!(
            key.as_str(),
            "contents"
                | "systemInstruction"
                | "tools"
                | "toolConfig"
                | "generationConfig"
                | "labels"
        )
    });
    body["aicode"] = json!({"experience":model});
    body["entitlement"] = json!({"userTier":license.user_tier});
    Ok((url, body))
}
