use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use codex_mixin::config::{GatewayConfig, load_stored_config, save_stored_config};
use codex_mixin::provider::ProviderModelSource;
use codex_mixin::provider::capabilities::ProviderCapabilities;
use codex_mixin::server::{AppState, ServeExit, serve_on_listener_with_reload};
use codex_mixin::web_search::WebSearchCapabilities;

use super::codex::{
    codex_home_path, managed_catalog_summary, reconcile_managed_skills,
    refresh_managed_codex_catalog_with_capabilities, refresh_managed_official_codex_catalog,
    resolve_codex_config_path, sync_managed_codex_gateway_base_url,
};
use super::runtime::{
    RuntimeMetadata, RuntimeMetadataGuard, config_fingerprint, delete_runtime_metadata,
    load_runtime_metadata, pid_is_running, save_runtime_metadata,
};

mod daemon;
mod logging;

#[cfg(test)]
pub(super) use daemon::running_daemon_needs_replacement;
pub(super) use daemon::{logs, restart, start_daemon, stop};
pub(super) use logging::init_tracing;
use logging::log_gateway_configuration;
#[cfg(test)]
pub(super) use logging::rotate_gateway_log_if_needed;

pub(super) const CODEX_CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(15);
pub(super) const OFFICIAL_CODEX_CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
pub(super) const PROVIDER_MODEL_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

struct ProviderModelRefreshTarget {
    id: String,
    display_name: String,
}

async fn sync_all_provider_models_once() -> bool {
    let Some(providers) = providers_or_log() else {
        return false;
    };
    let mut changed = false;
    for provider in providers {
        changed |= sync_provider_models(provider).await;
    }
    probe_all_stale_selected_models().await;
    refresh_client_models_after_change(changed).await;
    changed
}

async fn probe_all_stale_selected_models() {
    let Ok(config) = GatewayConfig::from_stored_config() else {
        return;
    };
    for provider in config.providers {
        probe_stale_selected_models(&provider.id).await;
    }
}

fn providers_or_log() -> Option<Vec<ProviderModelRefreshTarget>> {
    match dynamic_providers() {
        Ok(providers) => Some(providers),
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "failed to load providers for model sync");
            None
        }
    }
}

async fn refresh_client_models_after_change(changed: bool) {
    if !changed {
        return;
    }
    if let Err(error) = super::refresh_default_managed_codex_catalog().await {
        tracing::warn!(error = %format!("{error:#}"), "automatic Codex catalog refresh failed");
    }
    if let Err(error) = super::sync_installed_client_models() {
        tracing::warn!(error = %format!("{error:#}"), "automatic client model sync failed");
    }
}

fn dynamic_providers() -> anyhow::Result<Vec<ProviderModelRefreshTarget>> {
    let mut providers: Vec<ProviderModelRefreshTarget> = load_stored_config()?
        .map(|stored| {
            stored
                .providers
                .into_iter()
                .filter(|provider| !matches!(provider.model_source, ProviderModelSource::Static))
                .map(|provider| ProviderModelRefreshTarget {
                    display_name: if provider.display_name.trim().is_empty() {
                        provider.id.clone()
                    } else {
                        provider.display_name
                    },
                    id: provider.id,
                })
                .collect()
        })
        .unwrap_or_default();
    let config = GatewayConfig::from_stored_config()?;
    if config.accept_codex_oauth && config.codex_auth_path.is_file() {
        providers.insert(
            0,
            ProviderModelRefreshTarget {
                id: super::official_models::OFFICIAL_PROVIDER_ID.to_owned(),
                display_name: "OpenAI".to_owned(),
            },
        );
    }
    Ok(providers)
}

async fn sync_provider_models(provider: ProviderModelRefreshTarget) -> bool {
    let provider_id = provider.id;
    let changes = match super::providers::discover_models_with_output(&provider_id, true).await {
        Ok(changes) => changes,
        Err(error) => {
            tracing::warn!(
                provider_id,
                error = %format!("{error:#}"),
                "periodic provider model refresh failed"
            );
            return false;
        }
    };
    if provider_id != super::official_models::OFFICIAL_PROVIDER_ID {
        probe_auto_selected_models(&provider_id, &changes).await;
    }
    if changes.added.len() >= codex_mixin::provider::AUTO_SELECT_NEW_MODEL_LIMIT {
        tracing::warn!(
            provider_id,
            added = changes.added.len(),
            "model batch exceeds automatic selection limit"
        );
    }
    if changes.added.is_empty() && changes.removed.is_empty() {
        return false;
    }
    notify_model_changes(provider_id, provider.display_name, changes).await;
    true
}

#[allow(clippy::cognitive_complexity)]
async fn probe_stale_selected_models(provider_id: &str) {
    let config = match GatewayConfig::from_stored_config() {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(provider_id, error = %format!("{error:#}"), "automatic capability probe could not load config");
            return;
        }
    };
    let Some(provider) = config
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
    else {
        return;
    };
    let capabilities = match ProviderCapabilities::from_default_path(&config) {
        Ok(capabilities) => capabilities,
        Err(error) => {
            tracing::warn!(provider_id, error = %format!("{error:#}"), "automatic capability probe could not load cache");
            return;
        }
    };
    let models = provider
        .cached_models
        .iter()
        .filter(|model| {
            provider
                .selected_models
                .iter()
                .any(|selected| selected == &model.id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let model_ids = match capabilities.models_needing_probe(provider, &models) {
        Ok(model_ids) => model_ids,
        Err(error) => {
            tracing::warn!(provider_id, error = %format!("{error:#}"), "automatic capability probe could not inspect cache");
            return;
        }
    };
    if model_ids.is_empty() {
        return;
    }
    if let Err(error) = super::providers::probe_new_models(provider_id, &model_ids, false).await {
        tracing::warn!(provider_id, models = model_ids.join(","), error = %format!("{error:#}"), "automatic capability probe failed");
    }
}

async fn probe_auto_selected_models(
    provider_id: &str,
    changes: &codex_mixin::provider::ModelDiscoveryChanges,
) {
    if changes.auto_selected.is_empty() {
        return;
    }
    if let Err(error) =
        super::providers::probe_new_models(provider_id, &changes.auto_selected, false).await
    {
        tracing::warn!(
            provider_id,
            models = changes.auto_selected.join(","),
            error = %format!("{error:#}"),
            "automatic capability probe failed"
        );
    }
}

#[cfg(target_os = "macos")]
async fn notify_model_changes(
    provider_id: String,
    provider_display_name: String,
    changes: codex_mixin::provider::ModelDiscoveryChanges,
) {
    let Some(content) = model_notification_content(&provider_display_name, &changes) else {
        return;
    };
    let notification_provider_id = provider_id.clone();
    let notification = tokio::task::spawn_blocking(move || {
        deliver_model_notification(&notification_provider_id, &content)
    })
    .await;
    log_model_notification_result(&provider_id, notification);
}

#[cfg(target_os = "macos")]
fn deliver_model_notification(
    provider_id: &str,
    content: &ModelNotificationContent,
) -> io::Result<std::process::Output> {
    let helper = std::env::current_exe()
        .ok()
        .and_then(|executable| notification_helper_for(&executable))
        .filter(|path| path.is_file());
    if let Some(helper) = helper {
        return std::process::Command::new(helper)
            .args([
                "--deliver-model-notification",
                &content.title,
                &content.subtitle,
                &content.body,
            ])
            .output();
    }
    tracing::warn!(
        provider_id,
        "bundled notification helper unavailable; using generic macOS notification"
    );
    std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv\n display notification (item 3 of argv) with title (item 1 of argv) subtitle (item 2 of argv)\nend run",
            "--",
            &content.title,
            &content.subtitle,
            &content.body,
        ])
        .output()
}

#[cfg(target_os = "macos")]
fn log_model_notification_result(
    provider_id: &str,
    notification: Result<io::Result<std::process::Output>, tokio::task::JoinError>,
) {
    match notification {
        Ok(Ok(output)) if output.status.success() => {}
        Ok(Ok(output)) => tracing::warn!(
            provider_id,
            exit = ?output.status.code(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "macOS model notification failed"
        ),
        Ok(Err(error)) => {
            tracing::warn!(provider_id, error = %error, "macOS model notification failed")
        }
        Err(error) => tracing::warn!(provider_id, error = %error, "macOS notification task failed"),
    }
}

#[cfg(target_os = "macos")]
struct ModelNotificationContent {
    title: String,
    subtitle: String,
    body: String,
}

#[cfg(target_os = "macos")]
fn model_notification_content(
    provider_display_name: &str,
    changes: &codex_mixin::provider::ModelDiscoveryChanges,
) -> Option<ModelNotificationContent> {
    let mut lines = Vec::new();
    if !changes.auto_selected.is_empty() {
        lines.push(format!(
            "✓ 新增并探测 {} 个：{}",
            changes.auto_selected.len(),
            summarize_models(&changes.auto_selected)
        ));
    }
    if !changes.removed.is_empty() {
        lines.push(format!(
            "− 下线并移除 {} 个：{}",
            changes.removed.len(),
            summarize_models(&changes.removed)
        ));
    }
    (!lines.is_empty()).then(|| ModelNotificationContent {
        title: "Codex Mixin".to_owned(),
        subtitle: format!("{provider_display_name} · 模型列表已更新"),
        body: lines.join("\n"),
    })
}

#[cfg(target_os = "macos")]
fn summarize_models(models: &[String]) -> String {
    const DISPLAY_LIMIT: usize = 3;
    let mut summary = models
        .iter()
        .take(DISPLAY_LIMIT)
        .cloned()
        .collect::<Vec<_>>()
        .join("、");
    if models.len() > DISPLAY_LIMIT {
        summary.push_str(" 等");
    }
    summary
}

#[cfg(target_os = "macos")]
fn notification_helper_for(current_executable: &Path) -> Option<PathBuf> {
    let resources = current_executable.parent()?;
    if resources.file_name()?.to_str()? != "Resources" {
        return None;
    }
    let contents = resources.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    Some(contents.join("MacOS/CodexMixinMenu"))
}

#[cfg(all(test, target_os = "macos"))]
mod model_notification_tests {
    use super::*;
    use codex_mixin::provider::ModelDiscoveryChanges;

    #[test]
    fn formats_model_changes_for_native_notification_layout() {
        let changes = ModelDiscoveryChanges {
            added: vec!["gpt-5.6-luna".to_owned(), "gpt-6-astra".to_owned()],
            auto_selected: vec!["gpt-5.6-luna".to_owned(), "gpt-6-astra".to_owned()],
            removed: vec!["gpt-image-2".to_owned()],
        };

        let content = model_notification_content("我的常用模型", &changes)
            .expect("model changes should produce notification content");
        assert_eq!(content.title, "Codex Mixin");
        assert_eq!(content.subtitle, "我的常用模型 · 模型列表已更新");
        assert_eq!(
            content.body,
            "✓ 新增并探测 2 个：gpt-5.6-luna、gpt-6-astra\n− 下线并移除 1 个：gpt-image-2"
        );
    }

    #[test]
    fn truncates_long_model_lists_for_notification_banner() {
        let models = ["one", "two", "three", "four"].map(str::to_owned).to_vec();

        assert_eq!(summarize_models(&models), "one、two、three 等");
    }

    #[test]
    fn finds_notification_helper_only_inside_app_resources() {
        assert_eq!(
            notification_helper_for(Path::new(
                "/Applications/Codex Mixin.app/Contents/Resources/codex-mixin"
            )),
            Some(PathBuf::from(
                "/Applications/Codex Mixin.app/Contents/MacOS/CodexMixinMenu"
            ))
        );
        assert_eq!(
            notification_helper_for(Path::new("/usr/local/bin/codex-mixin")),
            None
        );
    }
}

#[cfg(not(target_os = "macos"))]
async fn notify_model_changes(
    _provider_id: String,
    _provider_display_name: String,
    _changes: codex_mixin::provider::ModelDiscoveryChanges,
) {
}

pub(super) fn persist_gateway_bind(bind: SocketAddr) -> anyhow::Result<bool> {
    let Some(mut stored) = load_stored_config()? else {
        return Ok(false);
    };
    let bind = bind.to_string();
    if stored.gateway_bind.as_deref() == Some(&bind) {
        return Ok(false);
    }
    stored.gateway_bind = Some(bind);
    save_stored_config(&stored)?;
    Ok(true)
}

pub(super) async fn bind_gateway_listener(
    bind: SocketAddr,
    automatic_bind: bool,
) -> anyhow::Result<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind(bind).await {
        Ok(listener) => Ok(listener),
        Err(err)
            if automatic_bind
                && bind.ip().is_loopback()
                && err.kind() == io::ErrorKind::AddrInUse =>
        {
            Ok(tokio::net::TcpListener::bind(SocketAddr::new(bind.ip(), 0)).await?)
        }
        Err(err) => Err(err.into()),
    }
}

#[allow(clippy::cognitive_complexity)]
pub(super) async fn start(
    bind: Option<SocketAddr>,
    daemon: bool,
    log_file: Option<PathBuf>,
) -> anyhow::Result<()> {
    let mut config = GatewayConfig::from_stored_config()?;
    let auxiliary_provider_enabled = config
        .providers
        .iter()
        .any(|provider| provider.enabled && provider.auxiliary_model_upstream);
    if let Err(error) = reconcile_managed_skills(&codex_home_path(), auxiliary_provider_enabled) {
        eprintln!(
            "warning: Codex Mixin skill guardian could not reconcile managed skills: {error:#}"
        );
    }
    let automatic_bind = bind.is_none();
    if let Some(bind) = bind {
        config.bind = bind;
    }
    log_gateway_configuration(&config);
    if daemon {
        return start_daemon(bind, log_file, &config, false);
    }
    if let Some(runtime) = load_runtime_metadata()? {
        if pid_is_running(runtime.pid)? {
            anyhow::bail!(
                "gateway already running: pid {}, bind {}",
                runtime.pid,
                runtime.bind
            );
        }
        tracing::warn!(pid = runtime.pid, "removing stale gateway runtime metadata");
        delete_runtime_metadata()?;
    }
    let listener = bind_gateway_listener(config.bind, automatic_bind).await?;
    let actual_bind = listener.local_addr()?;
    config.bind = actual_bind;
    if automatic_bind {
        persist_gateway_bind(actual_bind)?;
    }
    let config_path = resolve_codex_config_path(None)?;
    sync_managed_codex_gateway_base_url(&config_path, actual_bind)?;
    let supported_models = WebSearchCapabilities::from_default_path(&config)?.supported_model_ids();
    log_codex_catalog_refresh_started(&config_path, "gateway_start", "capability_cache");
    match refresh_managed_codex_catalog_with_capabilities(&config_path, Some(&supported_models)) {
        Ok(changed) => {
            log_codex_catalog_refresh(&config_path, "gateway_start", "capability_cache", changed)
        }
        Err(err) => tracing::warn!(
            trigger = "gateway_start",
            source = "capability_cache",
            error = %format!("{err:#}"),
            "failed to refresh Codex model catalog"
        ),
    }
    match crate::cli::sync_installed_client_models() {
        Ok(refreshed) if !refreshed.is_empty() => tracing::info!(
            clients = refreshed.join(", "),
            "refreshed connected client model catalogs"
        ),
        Ok(_) => {}
        Err(err) => tracing::warn!(
            error = %format!("{err:#}"),
            "failed to refresh connected client model catalogs"
        ),
    }
    let official_catalog_state = AppState::new(config.clone())?;
    let stop_provider_model_refresh = Arc::new(AtomicBool::new(false));
    let refresh_stop = Arc::clone(&stop_provider_model_refresh);
    let (reload_sender, mut reload_receiver) = tokio::sync::watch::channel(0_u64);
    let _provider_model_refresh_thread = std::thread::Builder::new()
        .name("provider-model-refresh".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(error = %error, "failed to start provider model refresh runtime");
                    return;
                }
            };
            runtime.block_on(async move {
                if sync_all_provider_models_once().await {
                    reload_sender
                        .send_modify(|revision| *revision = (*revision).wrapping_add(1));
                }
                let mut interval = tokio::time::interval(PROVIDER_MODEL_REFRESH_INTERVAL);
                interval.tick().await;
                while !refresh_stop.load(Ordering::Acquire) {
                    interval.tick().await;
                    if refresh_stop.load(Ordering::Acquire) {
                        break;
                    }
                    if sync_all_provider_models_once().await {
                        reload_sender
                            .send_modify(|revision| *revision = (*revision).wrapping_add(1));
                    }
                }
            });
        })
        .context("spawn provider model refresh thread")?;
    let capabilities_config_path = config_path.clone();
    let refresh_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(CODEX_CATALOG_REFRESH_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            log_codex_catalog_refresh_started(
                &capabilities_config_path,
                "periodic",
                "capability_cache",
            );
            let refresh_result = GatewayConfig::from_stored_config()
                .and_then(|current| WebSearchCapabilities::from_default_path(&current))
                .map(|capabilities| capabilities.supported_model_ids())
                .and_then(|supported_models| {
                    refresh_managed_codex_catalog_with_capabilities(
                        &capabilities_config_path,
                        Some(&supported_models),
                    )
                });
            match refresh_result {
                Ok(changed) => log_codex_catalog_refresh(
                    &capabilities_config_path,
                    "periodic",
                    "capability_cache",
                    changed,
                ),
                Err(err) => tracing::warn!(
                    trigger = "periodic",
                    source = "capability_cache",
                    error = %format!("{err:#}"),
                    "failed to refresh Codex model catalog"
                ),
            }
        }
    });
    let official_refresh_config = config.clone();
    let official_refresh_config_path = config_path.clone();
    let official_refresh_task = tokio::spawn(async move {
        refresh_official_codex_catalog(
            &official_refresh_config_path,
            &official_refresh_config,
            &official_catalog_state,
            "gateway_start",
        )
        .await;
        let mut interval = tokio::time::interval(OFFICIAL_CODEX_CATALOG_REFRESH_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            refresh_official_codex_catalog(
                &official_refresh_config_path,
                &official_refresh_config,
                &official_catalog_state,
                "periodic",
            )
            .await;
        }
    });
    let pid = std::process::id();
    save_runtime_metadata(&RuntimeMetadata {
        pid,
        bind: actual_bind,
        started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        config_fingerprint: config_fingerprint()?,
    })?;
    let _runtime_guard = RuntimeMetadataGuard { pid };
    let mut serve_config = config;
    let mut serve_listener = listener;
    let result = loop {
        match serve_on_listener_with_reload(serve_config, serve_listener, reload_receiver.clone())
            .await
        {
            Ok(ServeExit::Shutdown) => break Ok(()),
            Ok(ServeExit::Reload) => {
                reload_receiver.borrow_and_update();
                serve_config = GatewayConfig::from_stored_config()?;
                serve_config.bind = actual_bind;
                serve_listener = bind_gateway_listener(actual_bind, false).await?;
                save_runtime_metadata(&RuntimeMetadata {
                    pid,
                    bind: actual_bind,
                    started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                    version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                    config_fingerprint: config_fingerprint()?,
                })?;
                tracing::info!(%actual_bind, "gateway state reloaded after provider model change");
            }
            Err(error) => break Err(error),
        }
    };
    refresh_task.abort();
    official_refresh_task.abort();
    stop_provider_model_refresh.store(true, Ordering::Release);
    match &result {
        Ok(()) => tracing::info!(pid, "gateway stopped"),
        Err(error) => tracing::error!(
            pid,
            error = %format!("{error:#}"),
            "gateway stopped with error"
        ),
    }
    result
}

async fn refresh_official_codex_catalog(
    config_path: &Path,
    config: &GatewayConfig,
    state: &AppState,
    trigger: &'static str,
) {
    log_codex_catalog_refresh_started(config_path, trigger, "official_remote");
    let supported_models = match WebSearchCapabilities::from_default_path(config) {
        Ok(capabilities) => Some(capabilities.supported_model_ids()),
        Err(err) => {
            tracing::warn!(
                error = %format!("{err:#}"),
                "failed to load web search capabilities"
            );
            None
        }
    };
    match refresh_managed_official_codex_catalog(config_path, state, supported_models.as_ref())
        .await
    {
        Ok(changed) => log_codex_catalog_refresh(config_path, trigger, "official_remote", changed),
        Err(err) => tracing::warn!(
            trigger,
            source = "official_remote",
            error = %format!("{err:#}"),
            "failed to refresh official Codex model catalog"
        ),
    }
}

fn log_codex_catalog_refresh_started(config_path: &Path, trigger: &str, source: &str) {
    tracing::info!(
        trigger,
        source,
        config_path = %config_path.display(),
        "Codex model catalog refresh started"
    );
}

fn log_codex_catalog_refresh(config_path: &Path, trigger: &str, source: &str, changed: bool) {
    match managed_catalog_summary(config_path) {
        Ok(Some(summary)) => tracing::info!(
            trigger,
            source,
            changed,
            catalog_path = %summary.catalog_path.display(),
            mode = summary.mode,
            model_count = summary.model_count,
            managed_model_count = summary.managed_model_count,
            "Codex model catalog refresh completed"
        ),
        Ok(None) => tracing::info!(
            trigger,
            source,
            changed,
            config_path = %config_path.display(),
            "Codex model catalog refresh skipped; config is not managed"
        ),
        Err(error) => tracing::warn!(
            trigger,
            source,
            changed,
            config_path = %config_path.display(),
            error = %format!("{error:#}"),
            "Codex model catalog refreshed but summary could not be read"
        ),
    }
}
