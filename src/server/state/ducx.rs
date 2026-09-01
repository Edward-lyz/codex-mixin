use super::*;
use anyhow::Context;

impl AppState {
    pub(crate) async fn prewarm_ducx(&self) -> Result<(), GatewayError> {
        let reporting_providers = self
            .providers
            .providers()
            .iter()
            .filter(|provider| provider.uses_ducx_loopback() && provider.baidu_code_report())
            .collect::<Vec<_>>();
        if reporting_providers.is_empty() {
            if let Some(provider) = self
                .providers
                .providers()
                .iter()
                .find(|provider| provider.uses_ducx_loopback())
            {
                self.ducx_native_headers(provider).await?;
            }
            return Ok(());
        }

        for provider in reporting_providers {
            let provider_id = provider.id().to_owned();
            let runtime = self.ducx_runtime_for(provider).await?;
            match runtime
                .report_client_token(self.config.request_timeout)
                .await
            {
                Ok(token) => {
                    let result = tokio::task::spawn_blocking(move || {
                        crate::config::mutate_stored_config(|config| {
                            let stored_provider = config
                                .providers
                                .iter_mut()
                                .find(|candidate| candidate.id == provider_id)
                                .with_context(|| {
                                    format!("DUCX reporting provider disappeared: {provider_id}")
                                })?;
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
                    })
                    .await
                    .context("join DUCX report token persistence task");
                    if let Err(error) = result.and_then(|inner| inner) {
                        tracing::error!(
                            error = %format!("{error:#}"),
                            "failed to persist DUCX data-report client token"
                        );
                    }
                }
                Err(error) => {
                    tracing::error!(
                        error = %format!("{error:#}"),
                        "failed to warm up DUCX data-report client token"
                    );
                }
            }
            self.ducx_native_headers(provider).await?;
        }
        Ok(())
    }

    async fn ducx_runtime_for(
        &self,
        provider: &ProviderRuntime,
    ) -> Result<Arc<crate::ducx::DucxRuntime>, GatewayError> {
        let executable = provider
            .ducx_executable()
            .map(PathBuf::from)
            .or_else(crate::ducx::default_ducx_executable)
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
            crate::ducx::DucxRuntime::spawn(executable)
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
