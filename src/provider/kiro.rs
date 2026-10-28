use anyhow::{Context, ensure};
use serde_json::{Value, json};

use super::auth::kiro::KiroSession;
use super::{ProviderModel, ProviderRuntime};

struct CachedProfile {
    key: Vec<u8>,
    region: String,
    arn: String,
    fetched_at: std::time::Instant,
}

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
    let mut session = if !provider.definition().auth.api_key.trim().is_empty() {
        KiroSession {
            access_token: provider.definition().auth.api_key.clone(),
            profile_arn: None,
            region: region.to_owned(),
            token_type: super::auth::kiro::token_type(&provider.definition().auth.api_key, false),
        }
    } else {
        super::auth::kiro::session(region, force).await?
    };
    if let Some(arn) = std::env::var("KIRO_PROFILE_ARN")
        .ok()
        .filter(|arn| !arn.trim().is_empty())
    {
        let profile_region = arn
            .split(':')
            .nth(3)
            .filter(|region| !region.is_empty())
            .context("KIRO_PROFILE_ARN has no region")?;
        super::auth::kiro::validate_region(profile_region)?;
        session.region = profile_region.to_owned();
        session.profile_arn = Some(arn);
    }
    if session.profile_arn.is_none() {
        static CLIENT: tokio::sync::OnceCell<reqwest::Client> = tokio::sync::OnceCell::const_new();
        let client = CLIENT
            .get_or_try_init(|| async {
                crate::platform::proxy::auto_proxy(reqwest::Client::builder())
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(std::time::Duration::from_secs(20))
                    .build()
            })
            .await?;
        resolve_profile(client, &mut session).await?;
    }
    Ok(session)
}

async fn resolve_profile(
    client: &reqwest::Client,
    session: &mut KiroSession,
) -> anyhow::Result<()> {
    static CACHE: tokio::sync::Mutex<Vec<CachedProfile>> =
        tokio::sync::Mutex::const_new(Vec::new());
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    hash.update(session.region.as_bytes());
    hash.update(&[0]);
    hash.update(session.access_token.as_bytes());
    let key = hash.finish().as_ref().to_vec();
    {
        let mut cache = CACHE.lock().await;
        cache.retain(|entry| entry.fetched_at.elapsed() < std::time::Duration::from_secs(600));
        if let Some(entry) = cache.iter().find(|entry| entry.key == key) {
            session.region.clone_from(&entry.region);
            session.profile_arn = Some(entry.arn.clone());
            return Ok(());
        }
    }
    let mut regions = vec![session.region.clone()];
    if session.token_type == Some("API_KEY") {
        regions = vec!["us-east-1".to_owned()];
    } else {
        for region in ["us-east-1", "eu-central-1"] {
            if !regions.iter().any(|value| value == region) {
                regions.push(region.to_owned());
            }
        }
    }
    for region in regions {
        let mut request = client
            .post(format!("https://management.{region}.kiro.dev/"))
            .bearer_auth(&session.access_token);
        if let Some(kind) = session.token_type {
            request = request.header("tokentype", kind);
        }
        request = if session.token_type == Some("API_KEY") {
            request
                .header("x-amz-target", "AmazonCodeWhispererService.GetProfile")
                .header(reqwest::header::CONTENT_TYPE, "application/x-amz-json-1.0")
                .json(&json!({}))
        } else {
            request = client
                .post(format!(
                    "https://management.{region}.kiro.dev/List-Available-Profiles"
                ))
                .bearer_auth(&session.access_token);
            if let Some(kind) = session.token_type {
                request = request.header("tokentype", kind);
            }
            request.json(&json!({}))
        };
        let response = request.send().await?;
        if response.status() == reqwest::StatusCode::FORBIDDEN
            && session.token_type != Some("API_KEY")
        {
            continue;
        }
        ensure!(
            response.status().is_success(),
            "Kiro profile discovery returned {}",
            response.status()
        );
        let value = super::agy::bounded_json(response, "Kiro profiles").await?;
        let arn = value
            .pointer("/profile/arn")
            .and_then(Value::as_str)
            .or_else(|| {
                value
                    .get("profiles")
                    .and_then(Value::as_array)
                    .and_then(|profiles| {
                        profiles
                            .iter()
                            .find_map(|profile| profile.get("arn").and_then(Value::as_str))
                    })
            })
            .filter(|arn| !arn.is_empty());
        if let Some(arn) = arn {
            session.profile_arn = Some(arn.to_owned());
            session.region = region;
            let mut cache = CACHE.lock().await;
            if cache.len() >= 64 {
                cache.remove(0);
            }
            cache.push(CachedProfile {
                key,
                region: session.region.clone(),
                arn: arn.to_owned(),
                fetched_at: std::time::Instant::now(),
            });
            return Ok(());
        }
    }
    anyhow::bail!("Kiro account has no available profile")
}

pub(crate) async fn discover_models(
    client: &reqwest::Client,
    provider: &ProviderRuntime,
) -> anyhow::Result<Vec<ProviderModel>> {
    let session = credentials(provider, false).await?;
    let mut models = Vec::new();
    let mut pages = std::collections::HashSet::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut query = vec![
            ("origin", "KIRO_CLI"),
            ("profileArn", session.profile_arn.as_deref().unwrap_or("")),
        ];
        if let Some(next) = &next_token {
            query.push(("nextToken", next.as_str()));
        }
        let mut request = client
            .get(format!(
                "https://management.{}.kiro.dev/List-Available-Models",
                session.region
            ))
            .bearer_auth(&session.access_token)
            .query(&query)
            .timeout(std::time::Duration::from_secs(20));
        if let Some(kind) = session.token_type {
            request = request.header("tokentype", kind);
        }
        let response = request.send().await?;
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
        next_token = Some(next.to_owned());
    }
    ensure!(
        !models.is_empty(),
        "Kiro account returned an empty model directory"
    );
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    Ok(models)
}

/// Fetch Kiro credit balances for the CLI quota contract.
pub async fn fetch_kiro_quota(provider: &ProviderRuntime) -> anyhow::Result<Vec<Value>> {
    let session = credentials(provider, false).await?;
    let client = crate::platform::proxy::auto_proxy(reqwest::Client::builder())
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let mut request = client
        .get(format!(
            "https://management.{}.kiro.dev/Get-Usage-Limits",
            session.region
        ))
        .bearer_auth(&session.access_token)
        .query(&[
            ("origin", "KIRO_CLI"),
            ("resourceType", "CREDIT"),
            ("isEmailRequired", "false"),
            ("profileArn", session.profile_arn.as_deref().unwrap_or("")),
        ]);
    if let Some(kind) = session.token_type {
        request = request.header("tokentype", kind);
    }
    let response = request.send().await?;
    ensure!(
        response.status().is_success(),
        "Kiro usage query returned {}",
        response.status()
    );
    let value = super::agy::bounded_json(response, "Kiro usage").await?;
    parse_usage(&value)
}

fn parse_usage(value: &Value) -> anyhow::Result<Vec<Value>> {
    let buckets = value
        .get("usageBreakdownList")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty())
        .cloned()
        .or_else(|| {
            value
                .get("usageBreakdown")
                .filter(|v| v.is_object())
                .map(|v| vec![v.clone()])
        })
        .context("Kiro usage response has no credit breakdown")?;
    let mut rows = Vec::new();
    for (index, bucket) in buckets.iter().enumerate() {
        let id = bucket
            .get("resourceType")
            .and_then(Value::as_str)
            .unwrap_or("credits");
        let label = bucket
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or("Credits");
        let used = bucket
            .get("currentUsageWithPrecision")
            .or_else(|| bucket.get("currentUsage"))
            .and_then(Value::as_f64)
            .context("Kiro usage is missing current usage")?;
        let limit = bucket
            .get("usageLimitWithPrecision")
            .or_else(|| bucket.get("usageLimit"))
            .and_then(Value::as_f64)
            .context("Kiro usage is missing its limit")?;
        rows.push(json!({"quota_id": format!("{id}-{index}"), "label": label, "unit": "credits", "used": used, "limit": limit, "remaining": (limit-used).max(0.0), "reset_at": usage_date(bucket.get("nextDateReset").or_else(|| value.get("nextDateReset"))), "source": "kiro", "error": null}));
        if let Some(trial) = bucket.get("freeTrialInfo").filter(|v| v.is_object()) {
            let trial_used = trial
                .get("currentUsageWithPrecision")
                .or_else(|| trial.get("currentUsage"))
                .and_then(Value::as_f64);
            let trial_limit = trial
                .get("usageLimitWithPrecision")
                .or_else(|| trial.get("usageLimit"))
                .and_then(Value::as_f64);
            if let (Some(used), Some(limit)) = (trial_used, trial_limit) {
                rows.push(json!({"quota_id": format!("{id}-{index}-trial"), "label": "Trial credits", "unit": "credits", "used": used, "limit": limit, "remaining": (limit-used).max(0.0), "reset_at": usage_date(trial.get("freeTrialExpiry")), "source": "kiro", "error": null}));
            }
        }
    }
    Ok(rows)
}

fn usage_date(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(seconds) = value.as_i64() {
        return chrono::DateTime::from_timestamp(seconds, 0).map(|time| time.to_rfc3339());
    }
    value
        .as_str()
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
        .map(|date| date.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderPreset;

    #[test]
    fn parses_precise_credit_and_trial_balances_without_fabricating_usage() {
        let rows = parse_usage(
            &json!({"nextDateReset": 1735689600, "usageBreakdownList": [{
                "resourceType": "CREDIT", "displayName": "Credits", "currentUsage": 3,
                "currentUsageWithPrecision": 3.25, "usageLimit": 10,
                "freeTrialInfo": {"currentUsage": 1, "usageLimit": 5, "freeTrialExpiry": 1735689600}
            }]}),
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["used"], 3.25);
        assert_eq!(rows[0]["remaining"], 6.75);
        assert_eq!(rows[0]["reset_at"], "2025-01-01T00:00:00+00:00");
        assert_eq!(rows[1]["remaining"], 4.0);
        assert!(parse_usage(&json!({"usageBreakdownList": []})).is_err());
        assert!(parse_usage(&json!({"usageBreakdown": {"usageLimit": 10}})).is_err());
    }

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
