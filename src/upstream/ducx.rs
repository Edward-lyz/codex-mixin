use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use super::UpstreamAccess;
use crate::error::GatewayError;
use crate::provider::auth::ducx::DucxRuntime;
use crate::provider::{ProviderRegistry, ProviderRuntime};

impl UpstreamAccess {
    /// Warm up every enabled DUCX-loopback provider once before the gateway
    /// starts serving: persist data-report tokens and mint native headers.
    pub(crate) async fn prewarm_ducx(
        &self,
        providers: &ProviderRegistry,
    ) -> Result<(), GatewayError> {
        let ducx_providers = providers
            .providers()
            .iter()
            .filter(|provider| provider.definition().enabled && provider.uses_ducx_loopback())
            .collect::<Vec<_>>();
        for provider in ducx_providers {
            self.prewarm_ducx_report(provider).await?;
            self.ducx_native_headers(provider).await?;
        }
        Ok(())
    }

    async fn prewarm_ducx_report(&self, provider: &ProviderRuntime) -> Result<(), GatewayError> {
        if !provider.baidu_code_report() {
            return Ok(());
        }
        let provider_id = provider.id().to_owned();
        tracing::info!(provider_id = %provider_id, "starting DUCX data-report token warmup");
        let runtime = self.ducx_runtime_for(provider).await?;
        let Some(token) = self.capture_report_token(&runtime).await else {
            return Ok(());
        };
        tracing::info!(provider_id = %provider_id, "DUCX data-report token warmup captured a token");
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
                    "failed to warm up DUCX data-report client token"
                );
                None
            }
        }
    }

    async fn ducx_runtime_for(
        &self,
        provider: &ProviderRuntime,
    ) -> Result<Arc<DucxRuntime>, GatewayError> {
        let executable = provider
            .ducx_executable()
            .map(PathBuf::from)
            .or_else(crate::provider::auth::ducx::default_ducx_executable)
            .ok_or_else(|| {
                GatewayError::Upstream(
                    "DUCX loopback is enabled but the managed ducx executable was not found"
                        .to_owned(),
                )
            })?;
        let mut runtimes = self.ducx_runtimes.lock().await;
        if let Some(runtime) = runtimes.get(provider.id()) {
            return Ok(Arc::clone(runtime));
        }
        let runtime = Arc::new(
            DucxRuntime::spawn(executable)
                .await
                .map_err(GatewayError::Other)?,
        );
        runtimes.insert(provider.id().to_owned(), Arc::clone(&runtime));
        Ok(runtime)
    }

    /// Fetch the DUCX-native authentication headers (cached, minted on demand).
    pub(crate) async fn ducx_native_headers(
        &self,
        provider: &ProviderRuntime,
    ) -> Result<axum::http::HeaderMap, GatewayError> {
        self.ducx_runtime_for(provider)
            .await?
            .native_headers(self.config.request_timeout)
            .await
            .map_err(GatewayError::Other)
    }

    pub(crate) async fn invalidate_ducx_headers(
        &self,
        provider: &ProviderRuntime,
    ) -> Result<(), GatewayError> {
        self.ducx_runtime_for(provider)
            .await?
            .invalidate_headers()
            .await;
        Ok(())
    }

    pub(crate) async fn baidu_native_headers(
        &self,
        provider: &ProviderRuntime,
    ) -> Result<Option<axum::http::HeaderMap>, GatewayError> {
        if provider.uses_ducx_loopback() {
            Ok(Some(self.ducx_native_headers(provider).await?))
        } else {
            Ok(None)
        }
    }
}

async fn persist_report_token_async(provider_id: String, token: String) {
    let result =
        tokio::task::spawn_blocking(move || persist_ducx_report_token(&provider_id, token))
            .await
            .context("join DUCX report token persistence task");
    if let Err(error) = result.and_then(|inner| inner) {
        tracing::error!(
            error = %format!("{error:#}"),
            "failed to persist DUCX data-report client token"
        );
    }
}

fn persist_ducx_report_token(provider_id: &str, token: String) -> anyhow::Result<()> {
    crate::config::mutate_stored_config(|config| {
        let stored_provider = config
            .providers
            .iter_mut()
            .find(|candidate| candidate.id == provider_id)
            .with_context(|| format!("DUCX reporting provider disappeared: {provider_id}"))?;
        anyhow::ensure!(
            stored_provider.enabled
                && stored_provider.request_policy.baidu_code_report
                && stored_provider.request_policy.effective_baidu_auth_bridge()
                    == crate::provider::BaiduAuthBridge::DucxLoopback,
            "DUCX reporting provider changed during warmup: {provider_id}"
        );
        stored_provider.request_policy.data_report_client_token = Some(token);
        Ok(())
    })
}
