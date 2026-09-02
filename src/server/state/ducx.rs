use super::*;
use anyhow::Context;

impl AppState {
    pub(crate) async fn prewarm_ducx(&self) -> Result<(), GatewayError> {
        let ducx_providers = self
            .providers
            .providers()
            .iter()
            .filter(|provider| provider.definition().enabled && provider.uses_ducx_loopback())
            .collect::<Vec<_>>();
        for provider in ducx_providers {
            if provider.baidu_code_report() {
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
                                        format!(
                                            "DUCX reporting provider disappeared: {provider_id}"
                                        )
                                    })?;
                                anyhow::ensure!(
                                    stored_provider.enabled
                                        && stored_provider.request_policy.baidu_code_report
                                        && stored_provider
                                            .request_policy
                                            .effective_baidu_auth_bridge()
                                            == crate::provider::BaiduAuthBridge::DucxLoopback,
                                    "DUCX reporting provider changed during warmup: {provider_id}"
                                );
                                stored_provider.request_policy.data_report_client_token =
                                    Some(token);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GatewayConfig, ThinkingMode};
    use crate::provider::{BaiduAuthBridge, baidu_oneapi_provider};
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::Duration;

    #[tokio::test]
    async fn gateway_startup_prewarms_once_for_the_first_request() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let executable = home.join(".baidu-cx/baidu-cx/bin/ducx");
        let marker = home.join("native-warmup-count");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
/bin/echo warmup >> '{}'
/bin/sleep 0.1
for argument in "$@"; do
  case "$argument" in
    model_providers.oneapi.base_url=*)
      url=$(printf '%s' "$argument" | /usr/bin/sed -E 's/^[^"]*"([^"]+)".*/\1/')
      ;;
  esac
done
/usr/bin/curl --silent --max-time 2 \
  --header 'comate_custom_header: fixture' \
  --data '{{}}' "$url/responses" >/dev/null 2>&1 || true
"#,
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut provider = baidu_oneapi_provider("baidu", "key");
        provider.quota_username = Some("test-user".to_owned());
        provider.request_policy.baidu_auth_bridge = Some(BaiduAuthBridge::DucxLoopback);
        provider.request_policy.ducx_executable = Some(executable.clone());
        let state = AppState::new(GatewayConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            providers: vec![provider],
            official_responses_url: "https://example.invalid/responses".to_owned(),
            codex_auth_path: directory.path().join("auth.json"),
            gateway_api_key: None,
            gateway_client_keys: crate::gateway_access::GatewayClientKeys::default(),
            accept_codex_oauth: false,
            official_selected_models: None,
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_secs(3),
            thinking_mode: ThinkingMode::Off,
            enable_web_search_tool: false,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: None,
            fusion_profiles: Vec::new(),
        })
        .unwrap();

        assert!(state.providers.providers()[0].uses_ducx_loopback());
        assert_eq!(
            state.providers.providers()[0].ducx_executable(),
            Some(executable.as_path())
        );
        state.prewarm_ducx().await.unwrap();
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "warmup\n");

        let provider = &state.providers.providers()[0];
        state.ducx_native_headers(provider).await.unwrap();
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "warmup\n");
    }
}
