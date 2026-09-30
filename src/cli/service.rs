use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use codex_mixin::application::lifecycle::{
    GatewayLifecycleAction, GatewayObservation, gateway_lifecycle_action,
};
use codex_mixin::config::{GatewayConfig, load_stored_config, save_stored_config};
use codex_mixin::gateway_access::GatewayClientKeys;
use codex_mixin::platform::{
    DesktopNotification, StartupServiceSpec, StartupServiceStatus, install_startup_service,
    remove_startup_service, show_notification, start_startup_service, startup_service_status,
    stop_startup_service,
};
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
use super::status::gateway_snapshot;

mod daemon;
mod logging;

const GATEWAY_READINESS_ATTEMPTS: usize = 90;
const GATEWAY_READINESS_DELAY: Duration = Duration::from_secs(1);
const GATEWAY_STOP_ATTEMPTS: usize = 20;
const GATEWAY_STOP_DELAY: Duration = Duration::from_millis(250);

/// Run blocking OS-tool and process-control work off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .context("join gateway service task")?
}

/// Executable and log path the supervised gateway runs with.
#[derive(Clone)]
struct ServiceCommandLine {
    executable: PathBuf,
    log_file: PathBuf,
    config_path: PathBuf,
    codex_home: PathBuf,
}

impl ServiceCommandLine {
    fn current() -> anyhow::Result<Self> {
        Ok(Self {
            executable: std::env::current_exe().context("resolve gateway executable")?,
            log_file: super::runtime::default_log_file_path(),
            config_path: std::path::absolute(codex_mixin::config::stored_config_path())?,
            codex_home: std::path::absolute(codex_home_path())?,
        })
    }

    async fn status(&self) -> anyhow::Result<StartupServiceStatus> {
        let command = self.clone();
        blocking(move || startup_service_status(&command.spec())).await
    }

    async fn install(&self) -> anyhow::Result<()> {
        let command = self.clone();
        blocking(move || install_startup_service(&command.spec())).await
    }

    fn spec(&self) -> StartupServiceSpec<'_> {
        StartupServiceSpec {
            executable: &self.executable,
            log_file: &self.log_file,
            config_path: &self.config_path,
            codex_home: &self.codex_home,
        }
    }
}

/// Leave exactly one gateway running with this CLI's version, migrating a
/// daemon or stale startup definition under the service manager when the user
/// enabled startup at login. Also performs first-run model discovery for
/// providers that have never been refreshed.
pub(crate) async fn ensure_ready() -> anyhow::Result<()> {
    let config = GatewayConfig::from_stored_config().context("load gateway configuration")?;
    initialize_provider_models(&config).await;
    let command = ServiceCommandLine::current()?;
    let service = command.status().await?;
    let snapshot = gateway_snapshot(&config).await?;
    let action = gateway_lifecycle_action(GatewayObservation {
        gateway_healthy: snapshot.healthy(),
        gateway_version: snapshot.running_version.as_deref(),
        daemon_running: snapshot.daemon_running(),
        service_installed: service.installed,
        service_current: service.current,
        service_loaded: service.loaded,
        current_version: env!("CARGO_PKG_VERSION"),
    });
    match action {
        GatewayLifecycleAction::KeepReady => return Ok(()),
        GatewayLifecycleAction::RestartManaged | GatewayLifecycleAction::StartManaged => {
            restart_under_service(&command, service).await?;
        }
        GatewayLifecycleAction::RestartDaemon => {
            stop_all_gateways(service).await?;
            start_gateway_daemon(config.clone()).await?;
        }
        GatewayLifecycleAction::StartDaemon => start_gateway_daemon(config.clone()).await?,
    }
    wait_for_gateway_ready(&config).await
}

/// Start the gateway the way the user's autostart choice implies: through the
/// service manager when startup at login is enabled, otherwise as a daemon.
/// Never changes the autostart choice.
pub(crate) async fn start_managed() -> anyhow::Result<()> {
    ensure_ready().await
}

/// Stop every local gateway instance, keeping the startup definition.
pub(crate) async fn stop_managed() -> anyhow::Result<()> {
    let service = ServiceCommandLine::current()?.status().await?;
    stop_all_gateways(service).await
}

/// Restart under the service manager when autostart is enabled, otherwise as
/// a daemon, then wait for readiness.
pub(crate) async fn restart_managed() -> anyhow::Result<()> {
    let config = GatewayConfig::from_stored_config().context("load gateway configuration")?;
    let command = ServiceCommandLine::current()?;
    let service = command.status().await?;
    if service.installed {
        restart_under_service(&command, service).await?;
    } else {
        stop_all_gateways(service).await?;
        start_gateway_daemon(config.clone()).await?;
    }
    wait_for_gateway_ready(&config).await
}

/// Whether the gateway starts at login.
pub(crate) async fn autostart_enabled() -> anyhow::Result<bool> {
    Ok(ServiceCommandLine::current()?.status().await?.installed)
}

/// Enable or disable gateway startup at login. Enabling moves the gateway
/// under the service manager and starts it; disabling removes the definition
/// and keeps a previously running gateway running as a daemon.
pub(crate) async fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let command = ServiceCommandLine::current()?;
    let service = command.status().await?;
    if enabled {
        if service.installed && service.current {
            return ensure_ready().await;
        }
        let config = GatewayConfig::from_stored_config().context("load gateway configuration")?;
        stop_all_gateways(service).await?;
        command.install().await?;
        blocking(start_startup_service).await?;
        return wait_for_gateway_ready(&config).await;
    }
    if !service.installed && !service.loaded {
        return Ok(());
    }
    let config = match load_stored_config()? {
        Some(stored) if !stored.providers.is_empty() => Some(GatewayConfig::from_stored_config()?),
        _ => None,
    };
    let supervised_gateway_running = match &config {
        Some(config) => {
            let snapshot = gateway_snapshot(config).await?;
            snapshot.healthy() && !snapshot.daemon_running()
        }
        None => false,
    };
    blocking(remove_startup_service).await?;
    let Some(config) = config.filter(|_| supervised_gateway_running) else {
        return Ok(());
    };
    wait_for_gateway_stopped().await?;
    start_gateway_daemon(config.clone()).await?;
    wait_for_gateway_ready(&config).await
}

async fn restart_under_service(
    command: &ServiceCommandLine,
    service: StartupServiceStatus,
) -> anyhow::Result<()> {
    stop_all_gateways(service).await?;
    if !service.current {
        command.install().await?;
    }
    blocking(start_startup_service).await
}

/// Stop the service-manager job and any daemon or foreground gateway recorded
/// in runtime metadata, then wait until no recorded gateway process remains.
async fn stop_all_gateways(service: StartupServiceStatus) -> anyhow::Result<()> {
    if service.installed || service.loaded {
        blocking(stop_startup_service).await?;
    }
    blocking(|| daemon::stop_with_output(false, true)).await?;
    wait_for_gateway_stopped().await
}

async fn start_gateway_daemon(config: GatewayConfig) -> anyhow::Result<()> {
    blocking(move || start_daemon(None, None, &config, true)).await
}

async fn initialize_provider_models(config: &GatewayConfig) {
    for provider in config.providers.iter().filter(|provider| {
        provider.enabled
            && provider.cached_models.is_empty()
            && provider.models_refreshed_at_ms.is_none()
    }) {
        if let Err(error) = super::providers::discover_models_with_output(&provider.id, true).await
        {
            tracing::warn!(
                provider_id = %provider.id,
                error = %format!("{error:#}"),
                "initial provider model discovery failed"
            );
        }
    }
}

async fn wait_for_gateway_ready(config: &GatewayConfig) -> anyhow::Result<()> {
    let mut last_failure = "gateway has not reported a healthy status".to_owned();
    for attempt in 0..GATEWAY_READINESS_ATTEMPTS {
        match gateway_snapshot(config).await {
            Ok(snapshot) => match snapshot.health_error {
                None if snapshot.running_version.as_deref() == Some(env!("CARGO_PKG_VERSION")) => {
                    return Ok(());
                }
                None => {
                    last_failure = format!(
                        "gateway version is {}; expected {}",
                        snapshot.running_version.as_deref().unwrap_or("unknown"),
                        env!("CARGO_PKG_VERSION")
                    )
                }
                Some(error) => last_failure = error,
            },
            Err(error) => last_failure = format!("{error:#}"),
        }
        if attempt + 1 < GATEWAY_READINESS_ATTEMPTS {
            tokio::time::sleep(GATEWAY_READINESS_DELAY).await;
        }
    }
    anyhow::bail!("gateway did not become ready within 90 seconds: {last_failure}")
}

async fn wait_for_gateway_stopped() -> anyhow::Result<()> {
    for _ in 0..GATEWAY_STOP_ATTEMPTS {
        let recorded = [
            load_runtime_metadata()?.map(|runtime| runtime.pid),
            super::runtime::load_daemon_metadata()?.map(|daemon| daemon.pid),
        ];
        let mut running = false;
        for pid in recorded.into_iter().flatten() {
            running |= pid_is_running(pid)?;
        }
        if !running {
            return Ok(());
        }
        tokio::time::sleep(GATEWAY_STOP_DELAY).await;
    }
    anyhow::bail!("gateway did not stop within 5 seconds; refusing to start a duplicate")
}

#[cfg(test)]
pub(super) use daemon::running_daemon_needs_replacement;
pub(super) use daemon::{logs, restart, start_daemon, stop};
pub(super) use logging::init_tracing;
use logging::log_gateway_configuration;

pub(super) const CODEX_CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(15);
pub(super) const OFFICIAL_CODEX_CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
pub(super) const PROVIDER_MODEL_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// How often the client-credential watch checks the stored configuration timestamp.
const CLIENT_CREDENTIAL_WATCH_INTERVAL: Duration = Duration::from_secs(2);

struct ProviderModelRefreshTarget {
    id: String,
    display_name: String,
}

/// Credential material a serving gateway authenticates requests against.
///
/// `connect remove <client>` revokes a client key and the next `connect` mints a
/// fresh one, but a running gateway keeps the credentials it was started with.
/// Without a reload it keeps rejecting the key it just handed the client, and
/// every request from that client fails `401 unauthorized` until the gateway is
/// restarted.
type ClientCredentials = (Option<String>, GatewayClientKeys);

fn stored_client_credentials() -> anyhow::Result<ClientCredentials> {
    let stored = load_stored_config()?.unwrap_or_default();
    Ok((stored.gateway_api_key, stored.gateway_client_keys))
}

/// Reload the gateway when the stored client credentials no longer match the
/// ones it serves.
///
/// A `connect` command runs in its own process, so it can only report the key it
/// wrote; the serving gateway has to notice the change itself. Reading the
/// document is gated on the config file's timestamp, so a file nobody rewrote
/// costs one `stat` per tick while a rotation reaches the gateway within a
/// couple of seconds.
fn spawn_client_credential_watch(
    served: ClientCredentials,
    stop: Arc<AtomicBool>,
    reload: tokio::sync::watch::Sender<u64>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("client-credential-watch".to_owned())
        .spawn(move || {
            let mut credentials = served;
            let mut fingerprint = config_fingerprint().ok().flatten();
            while !stop.load(Ordering::Acquire) {
                std::thread::sleep(CLIENT_CREDENTIAL_WATCH_INTERVAL);
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let current = config_fingerprint().ok().flatten();
                if current == fingerprint {
                    continue;
                }
                fingerprint = current;
                match stored_client_credentials() {
                    Ok(next) if next != credentials => {
                        credentials = next;
                        tracing::info!(
                            "gateway client credentials changed; reloading to serve the stored keys"
                        );
                        reload.send_modify(|revision| *revision = (*revision).wrapping_add(1));
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        error = %format!("{error:#}"),
                        "failed to read gateway client credentials"
                    ),
                }
            }
        })
        .map(|_| ())
}

async fn sync_all_provider_models_once() -> bool {
    let Some(providers) = providers_or_log() else {
        return false;
    };
    let mut changed = false;
    for provider in providers {
        changed |= sync_provider_models(provider).await;
    }
    probe_all_missing_selected_models().await;
    refresh_client_models_after_change(changed).await;
    changed
}

async fn probe_all_missing_selected_models() {
    let Ok(config) = GatewayConfig::from_stored_config() else {
        return;
    };
    // Probes send real completions, so a disabled provider never spends the
    // user's balance in the background.
    for provider in config.providers.iter().filter(|provider| provider.enabled) {
        probe_missing_selected_models(&provider.id).await;
    }
}

/// Providers the background loop refreshes. Disabled providers are skipped
/// entirely: refreshing would auto-select new models and probe them with
/// paid requests against an account the user switched off.
fn model_refresh_targets(
    providers: &[codex_mixin::provider::ProviderDefinition],
) -> Vec<ProviderModelRefreshTarget> {
    providers
        .iter()
        .filter(|provider| provider.enabled)
        .filter(|provider| !matches!(provider.model_source, ProviderModelSource::Static))
        .map(|provider| ProviderModelRefreshTarget {
            display_name: if provider.display_name.trim().is_empty() {
                provider.id.clone()
            } else {
                provider.display_name.clone()
            },
            id: provider.id.clone(),
        })
        .collect()
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
    let mut providers = load_stored_config()?
        .map(|stored| model_refresh_targets(&stored.providers))
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
async fn probe_missing_selected_models(provider_id: &str) {
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

async fn notify_model_changes(
    provider_id: String,
    provider_display_name: String,
    changes: codex_mixin::provider::ModelDiscoveryChanges,
) {
    let Some(notification) = model_notification(&provider_display_name, &changes) else {
        return;
    };
    let delivery = tokio::task::spawn_blocking(move || show_notification(&notification)).await;
    match delivery {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::warn!(
            provider_id,
            error = %format!("{error:#}"),
            "model change notification failed"
        ),
        Err(error) => {
            tracing::warn!(provider_id, error = %error, "model change notification task failed")
        }
    }
}

fn model_notification(
    provider_display_name: &str,
    changes: &codex_mixin::provider::ModelDiscoveryChanges,
) -> Option<DesktopNotification> {
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
    (!lines.is_empty()).then(|| DesktopNotification {
        title: "Codex Mixin".to_owned(),
        subtitle: format!("{provider_display_name} · 模型列表已更新"),
        body: lines.join("\n"),
    })
}

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
    // Service start is an explicit client-integration point: push the current
    // gateway client key into every installed client. A corrupted client must
    // not block the gateway from starting; report it and continue.
    if let Err(error) = super::sync_installed_client_keys() {
        tracing::error!(
            error = %format!("{error:#}"),
            "failed to sync installed client gateway keys; run codex-mixin doctor to repair"
        );
    }
    // Client sync can create and persist a missing dedicated key. Build every
    // server state from that committed configuration while preserving the
    // listener's actual address selected above.
    config = codex_mixin::application::lifecycle::reload_for_listener(actual_bind)?;
    let supported_models = WebSearchCapabilities::from_default_path(&config)?.supported_model_ids();
    let auto_review_slug = codex_mixin::provider::auxiliary_auto_review_slug(&config.providers);
    let official_catalog_state = AppState::new(config.clone())?;
    let stop_provider_model_refresh = Arc::new(AtomicBool::new(false));
    let refresh_stop = Arc::clone(&stop_provider_model_refresh);
    let (reload_sender, mut reload_receiver) = tokio::sync::watch::channel(0_u64);
    let served_credentials = (
        config.gateway_api_key.clone(),
        config.gateway_client_keys.clone(),
    );
    let stop_client_credential_watch = Arc::new(AtomicBool::new(false));
    spawn_client_credential_watch(
        served_credentials,
        Arc::clone(&stop_client_credential_watch),
        reload_sender.clone(),
    )
    .context("spawn client credential watch thread")?;
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
                .and_then(|current| {
                    let capabilities = WebSearchCapabilities::from_default_path(&current)?;
                    Ok((
                        capabilities.supported_model_ids(),
                        codex_mixin::provider::auxiliary_auto_review_slug(&current.providers),
                    ))
                })
                .and_then(|(supported_models, auto_review_slug)| {
                    refresh_managed_codex_catalog_with_capabilities(
                        &capabilities_config_path,
                        Some(&supported_models),
                        auto_review_slug.as_deref(),
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
    let startup_config_path = config_path.clone();
    // Catalog sync reads configuration repeatedly. On Windows each protected
    // read can launch an ACL process; keep this off the readiness path.
    let startup_catalog_task = tokio::task::spawn_blocking(move || {
        log_codex_catalog_refresh_started(
            &startup_config_path,
            "gateway_start",
            "capability_cache",
        );
        match refresh_managed_codex_catalog_with_capabilities(
            &startup_config_path,
            Some(&supported_models),
            auto_review_slug.as_deref(),
        ) {
            Ok(changed) => log_codex_catalog_refresh(
                &startup_config_path,
                "gateway_start",
                "capability_cache",
                changed,
            ),
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
    });
    let startup_catalog_task = tokio::spawn(async move {
        if let Err(error) = startup_catalog_task.await {
            tracing::error!(error = %error, "startup model catalog task failed");
        }
    });
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
                tracing::info!(%actual_bind, "gateway state reloaded from stored configuration");
            }
            Err(error) => break Err(error),
        }
    };
    refresh_task.abort();
    official_refresh_task.abort();
    startup_catalog_task.abort();
    stop_provider_model_refresh.store(true, Ordering::Release);
    stop_client_credential_watch.store(true, Ordering::Release);
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
fn providers_or_log() -> Option<Vec<ProviderModelRefreshTarget>> {
    match dynamic_providers() {
        Ok(providers) => Some(providers),
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "failed to load providers for model sync");
            None
        }
    }
}

#[cfg(test)]
mod model_refresh_target_tests {
    use super::*;

    #[test]
    fn background_refresh_skips_disabled_and_static_providers() {
        let mut enabled = codex_mixin::provider::custom_provider("enabled", "key");
        enabled.enabled = true;
        let mut disabled = codex_mixin::provider::custom_provider("openrouter", "key");
        disabled.enabled = false;
        let mut static_models = codex_mixin::provider::custom_provider("static", "key");
        static_models.enabled = true;
        static_models.model_source = ProviderModelSource::Static;

        let targets = model_refresh_targets(&[enabled, disabled, static_models]);

        assert_eq!(
            targets
                .iter()
                .map(|target| target.id.as_str())
                .collect::<Vec<_>>(),
            ["enabled"]
        );
    }
}

#[cfg(test)]
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

        let content = model_notification("我的常用模型", &changes)
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
}
