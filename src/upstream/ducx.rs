use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use super::UpstreamAccess;
use crate::error::GatewayError;
use crate::provider::auth::ducx::DucxRuntime;
use crate::provider::{ProviderRegistry, ProviderRuntime};

impl UpstreamAccess {
    /// Warm up every enabled managed-auth provider once before the gateway
    /// starts serving: persist telemetry tokens and mint native headers.
    pub(crate) async fn prewarm_providers(
        &self,
        providers: &ProviderRegistry,
    ) -> Result<(), GatewayError> {
        let managed_providers = providers
            .providers()
            .iter()
            .filter(|provider| provider.definition().enabled && provider.uses_managed_auth_bridge())
            .collect::<Vec<_>>();
        for provider in managed_providers {
            self.prewarm_telemetry(provider).await?;
            self.managed_native_headers(provider).await?;
        }
        Ok(())
    }

    async fn prewarm_telemetry(&self, provider: &ProviderRuntime) -> Result<(), GatewayError> {
        if !provider.reports_usage_telemetry() {
            return Ok(());
        }
        let provider_id = provider.id().to_owned();
        tracing::info!(provider_id = %provider_id, "starting usage telemetry token warmup");
        let runtime = self.managed_auth_runtime_for(provider).await?;
        let Some(token) = self.capture_report_token(&runtime).await else {
            return Ok(());
        };
        tracing::info!(provider_id = %provider_id, "usage telemetry token warmup captured a token");
        persist_report_token_async(provider_id, token).await;
        Ok(())
    }

    async fn capture_report_token(&self, runtime: &DucxRuntime) -> Option<String> {
        match runtime
            .report_client_token(self.config.request_timeout)
            .await
        {
            Ok(token) => Some(token),
            Err(error) => {
                tracing::error!(
                    error = %format!("{error:#}"),
                    "failed to warm up managed auth telemetry client token"
                );
                None
            }
        }
    }

    async fn managed_auth_runtime_for(
        &self,
        provider: &ProviderRuntime,
    ) -> Result<Arc<DucxRuntime>, GatewayError> {
        let executable = provider
            .auth_bridge_executable()
            .map(PathBuf::from)
            .or_else(crate::provider::auth::ducx::default_ducx_executable)
            .ok_or_else(|| {
                GatewayError::Upstream(
                    "managed auth bridge is enabled but its executable was not found".to_owned(),
                )
            })?;
        let mut runtimes = self.managed_auth_runtimes.lock().await;
        if let Some(runtime) = runtimes.get(provider.id()) {
            return Ok(Arc::clone(runtime));
        }
        let runtime = DucxRuntime::shared(executable).map_err(GatewayError::Other)?;
        runtimes.insert(provider.id().to_owned(), Arc::clone(&runtime));
        Ok(runtime)
    }

    /// Fetch the bridge-native authentication headers (cached, minted on demand).
    pub(crate) async fn managed_native_headers(
        &self,
        provider: &ProviderRuntime,
    ) -> Result<axum::http::HeaderMap, GatewayError> {
        self.managed_auth_runtime_for(provider)
            .await?
            .native_headers(self.config.request_timeout)
            .await
            .map_err(GatewayError::Other)
    }

    pub(crate) async fn invalidate_native_auth_headers(
        &self,
        provider: &ProviderRuntime,
    ) -> Result<(), GatewayError> {
        self.managed_auth_runtime_for(provider)
            .await?
            .invalidate_headers()
            .await;
        Ok(())
    }

    pub(crate) async fn native_auth_headers(
        &self,
        provider: &ProviderRuntime,
    ) -> Result<Option<axum::http::HeaderMap>, GatewayError> {
        if provider.uses_managed_auth_bridge() {
            Ok(Some(self.managed_native_headers(provider).await?))
        } else {
            Ok(None)
        }
    }
}

async fn persist_report_token_async(provider_id: String, token: String) {
    let result = tokio::task::spawn_blocking(move || persist_report_token(&provider_id, token))
        .await
        .context("join telemetry token persistence task");
    if let Err(error) = result.and_then(|inner| inner) {
        tracing::error!(
            error = %format!("{error:#}"),
            "failed to persist usage telemetry client token"
        );
    }
}

fn persist_report_token(provider_id: &str, token: String) -> anyhow::Result<()> {
    crate::config::mutate_stored_config(|config| {
        let stored_provider = config
            .providers
            .iter_mut()
            .find(|candidate| candidate.id == provider_id)
            .with_context(|| format!("telemetry provider disappeared: {provider_id}"))?;
        anyhow::ensure!(
            stored_provider.enabled
                && crate::provider::provider_reports_usage_telemetry(stored_provider)
                && crate::provider::provider_uses_managed_auth_bridge(stored_provider),
            "telemetry provider changed during warmup: {provider_id}"
        );
        stored_provider.request_policy.data_report_client_token = Some(token);
        Ok(())
    })
}
