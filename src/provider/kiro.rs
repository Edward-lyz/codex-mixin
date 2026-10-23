use anyhow::{Context, ensure};
use serde_json::{Value, json};

use super::auth::kiro::KiroSession;
use super::{ProviderModel, ProviderRuntime};

pub(crate) fn region(provider: &ProviderRuntime) -> anyhow::Result<&str> {
    let url = provider.api_url();
    let region = url
        .host_str()
        .and_then(|host| host.strip_prefix("runtime."))
        .and_then(|host| host.strip_suffix(".kiro.dev"))
        .context("Kiro native authentication requires a runtime.<region>.kiro.dev endpoint")?;
    super::auth::kiro::validate_region(region)?;
    ensure!(
        url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "Kiro native authentication requires its official HTTPS root endpoint"
    );
    Ok(region)
}

pub(crate) async fn credentials(
    provider: &ProviderRuntime,
    force: bool,
) -> anyhow::Result<KiroSession> {
    let region = region(provider)?;
    if !provider.definition().auth.api_key.trim().is_empty() {
        return Ok(KiroSession {
            access_token: provider.definition().auth.api_key.clone(),
            profile_arn: None,
            region: region.to_owned(),
        });
    }
    super::auth::kiro::session(region, force).await
}

pub(crate) async fn discover_models(
    client: &reqwest::Client,
    provider: &ProviderRuntime,
) -> anyhow::Result<Vec<ProviderModel>> {
    let session = credentials(provider, false).await?;
    let mut body = json!({"origin":"AI_EDITOR"});
    if let Some(arn) = &session.profile_arn {
        body["profileArn"] = json!(arn);
    }
    let mut models = Vec::new();
    let mut pages = std::collections::HashSet::new();
    loop {
        let response = client
            .post(format!("https://management.{}.kiro.dev/", session.region))
            .bearer_auth(&session.access_token)
            .header(
                "x-amz-target",
                "KiroControlPlaneBearerService.ListAvailableModels",
            )
            .header(reqwest::header::CONTENT_TYPE, "application/x-amz-json-1.0")
            .timeout(std::time::Duration::from_secs(20))
            .json(&body)
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "Kiro model discovery returned {}",
            response.status()
        );
        let value = super::agy::bounded_json(response, "Kiro model directory").await?;
        let entries = value
            .get("models")
            .and_then(Value::as_array)
            .context("Kiro model directory is missing models")?;
        for entry in entries {
            let id = entry
                .get("modelId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .context("Kiro modelId is missing")?;
            models.push(ProviderModel {
                id: id.to_owned(),
                display_name: entry
                    .get("modelName")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                description: entry
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                context_window: entry
                    .pointer("/tokenLimits/maxInputTokens")
                    .and_then(Value::as_u64),
                max_output_tokens: entry
                    .pointer("/tokenLimits/maxOutputTokens")
                    .and_then(Value::as_u64),
                supports_image: entry.get("supportsImages").and_then(Value::as_bool),
                supports_function_tools: entry.get("supportsTools").and_then(Value::as_bool),
                ..Default::default()
            });
        }
        ensure!(
            models.len() <= 10_000,
            "Kiro model directory exceeds 10000 models"
        );
        let Some(next) = value
            .get("nextToken")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
        else {
            break;
        };
        ensure!(
            pages.len() < 100 && pages.insert(next.to_owned()),
            "Kiro model pagination repeated or exceeds 100 pages"
        );
        body["nextToken"] = json!(next);
    }
    ensure!(
        !models.is_empty(),
        "Kiro account returned an empty model directory"
    );
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderPreset;

    #[test]
    fn native_credentials_require_official_https_root() {
        for (base_url, path, allowed) in [
            ("https://runtime.us-east-1.kiro.dev", "/", true),
            ("https://runtime.eu-west-1.kiro.dev", "/", true),
            ("http://runtime.us-east-1.kiro.dev", "/", false),
            (
                "https://runtime.us-east-1.kiro.dev.evil.example",
                "/",
                false,
            ),
            ("https://runtime.us-east-1.kiro.dev:8443", "/", false),
            ("https://runtime.us-east-1.kiro.dev", "/v1/messages", false),
            (
                "https://runtime.us-east-1.kiro.dev",
                "/?redirect=evil",
                false,
            ),
        ] {
            let mut definition = ProviderPreset::Kiro.create("kiro", "");
            definition.base_url = base_url.to_owned();
            definition.api_path = path.to_owned();
            let accepted = ProviderRuntime::new(definition, &|_| None)
                .and_then(|provider| region(&provider).map(|_| ()))
                .is_ok();
            assert_eq!(accepted, allowed, "{base_url}{path}");
        }
    }
}
