use std::time::{Duration, Instant};

use codex_mixin::provider::{
    ProviderDefinition, ProviderModelSource, discover_provider_models, redact_provider_error,
};
use futures_util::future::join_all;

use super::{DoctorProviderCheck, DoctorStatus};

pub(super) async fn check_doctor_providers(
    providers: Vec<ProviderDefinition>,
    timeout: Duration,
    probe_live: bool,
) -> anyhow::Result<Vec<DoctorProviderCheck>> {
    let client = reqwest::Client::builder().timeout(timeout).build()?;
    Ok(join_all(
        providers
            .into_iter()
            .map(|provider| check_doctor_provider(client.clone(), provider, probe_live)),
    )
    .await)
}

pub(super) async fn check_doctor_provider(
    client: reqwest::Client,
    provider: ProviderDefinition,
    probe_live: bool,
) -> DoctorProviderCheck {
    let readiness = provider.readiness();
    let base = DoctorProviderCheck {
        provider_id: provider.id.clone(),
        display_name: provider.display_name.clone(),
        enabled: provider.enabled,
        protocol: format!("{:?}", provider.protocol),
        status: DoctorStatus::Ok,
        selected_model_count: provider.selected_models.len(),
        routable_model_count: readiness.routable_model_count,
        message: String::new(),
        detail: None,
        paid_inference_performed: false,
    };
    if let Err(error) = provider.validate() {
        return DoctorProviderCheck {
            status: DoctorStatus::Error,
            message: "provider configuration failed validation".to_owned(),
            detail: Some(format!("{error:#}")),
            ..base
        };
    }
    if !provider.enabled {
        return DoctorProviderCheck {
            status: DoctorStatus::Warning,
            message: "provider is disabled; skipped network checks".to_owned(),
            ..base
        };
    }
    if readiness.routable_model_count == 0 {
        return DoctorProviderCheck {
            status: DoctorStatus::Error,
            message: "no selected models are currently available".to_owned(),
            detail: Some(readiness.issues.join(", ")),
            ..base
        };
    }
    if provider.model_source == ProviderModelSource::Static {
        return DoctorProviderCheck {
            message: format!(
                "static model source is healthy with {} routable model(s); no paid inference was performed",
                readiness.routable_model_count
            ),
            ..base
        };
    }
    if !probe_live {
        let cached_model_count = provider.cached_models.len();
        let refreshed = provider
            .models_refreshed_at_ms
            .map(|timestamp| format!("; cache timestamp {timestamp}"))
            .unwrap_or_default();
        return DoctorProviderCheck {
            status: if provider.models_refresh_error.is_some() {
                DoctorStatus::Warning
            } else {
                DoctorStatus::Ok
            },
            message: format!(
                "quick check used {} cached model(s) without contacting upstream{refreshed}",
                cached_model_count
            ),
            detail: provider.models_refresh_error.clone(),
            ..base
        };
    }
    let started = Instant::now();
    match discover_provider_models(&client, &provider).await {
        Ok(models) => DoctorProviderCheck {
            message: format!(
                "models endpoint healthy; returned {} model(s) in {} ms; no paid inference was performed",
                models.len(),
                started.elapsed().as_millis()
            ),
            detail: provider.models_refresh_error.as_ref().map(|error| {
                format!(
                    "a previous refresh error is still cached, but this check recovered: {error}"
                )
            }),
            ..base
        },
        Err(error) => {
            let error = redact_provider_error(&provider, &format!("{error:#}"));
            tracing::warn!(
                provider_id = %provider.id,
                error = %error,
                "doctor provider model discovery failed"
            );
            DoctorProviderCheck {
                status: DoctorStatus::Error,
                message: "models endpoint connection or response check failed".to_owned(),
                detail: Some(error),
                ..base
            }
        }
    }
}
