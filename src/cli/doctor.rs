use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use codex_mixin::CODEX_MIXIN_PROVIDER;
use codex_mixin::config::{GatewayConfig, load_stored_config, stored_config_path};
use codex_mixin::provider::{
    ProviderDefinition, ProviderModelSource, discover_provider_models, redact_provider_error,
};
use console::style;
use futures_util::future::join_all;
use serde::Serialize;
use toml_edit::{DocumentMut, Item};

use super::codex::{
    is_managed_catalog_model, is_managed_config, managed_catalog_path, managed_config_provider_id,
    refresh_default_managed_codex_catalog, resolve_codex_cli, resolve_codex_config_path,
    sync_managed_codex_gateway_base_url,
};
use super::runtime::*;
use super::service::start_daemon;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DoctorStatus {
    Ok,
    Warning,
    Error,
}

impl DoctorStatus {
    fn icon(self) -> console::StyledObject<&'static str> {
        match self {
            Self::Ok => style("✓").green().bold(),
            Self::Warning => style("⚠").yellow().bold(),
            Self::Error => style("✗").red().bold(),
        }
    }
}

/// Automatic repairs that `doctor --fix` can apply. Variant order equals
/// application order: gateway state first, then managed config, apps last.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum DoctorFix {
    CleanStaleGatewayMetadata,
    StartGateway,
    FixConfigPermissions,
    SyncGatewayBaseUrl,
    RefreshCodexCatalog,
    RestartChatGptApp,
    RestartCodexApp,
}

impl DoctorFix {
    /// Restarting a desktop app can kill in-flight user sessions, so it is
    /// only applied when the user explicitly passes `--restart-apps`.
    fn requires_restart_opt_in(self) -> bool {
        matches!(self, Self::RestartChatGptApp | Self::RestartCodexApp)
    }

    fn description(self) -> &'static str {
        match self {
            Self::CleanStaleGatewayMetadata => "Remove stale gateway runtime metadata",
            Self::StartGateway => "Start the gateway daemon",
            Self::FixConfigPermissions => "Tighten config file permissions to 600",
            Self::SyncGatewayBaseUrl => "Sync the managed Codex gateway base_url",
            Self::RefreshCodexCatalog => "Regenerate the managed model catalog",
            Self::RestartChatGptApp => "Restart the ChatGPT app to load the new config",
            Self::RestartCodexApp => "Restart the Codex app to load the new config",
        }
    }
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    id: String,
    name: String,
    status: DoctorStatus,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix_hint: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    auto_fixes: Vec<DoctorFix>,
}

impl DoctorCheck {
    fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        status: DoctorStatus,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            status,
            message: message.into(),
            detail: None,
            fix_hint: None,
            auto_fixes: Vec::new(),
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.fix_hint = Some(hint.into());
        self
    }

    fn fix(mut self, fix: DoctorFix) -> Self {
        self.auto_fixes.push(fix);
        self
    }
}

#[derive(Debug, Serialize)]
struct DoctorProviderCheck {
    provider_id: String,
    display_name: String,
    enabled: bool,
    protocol: String,
    status: DoctorStatus,
    selected_model_count: usize,
    routable_model_count: usize,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    paid_inference_performed: bool,
}

#[derive(Debug, Serialize)]
struct DoctorSummary {
    ok: usize,
    warnings: usize,
    errors: usize,
}

#[derive(Debug, Serialize)]
struct RepairOutcome {
    fix: DoctorFix,
    description: String,
    ok: bool,
    message: String,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    ok: bool,
    generated_at_ms: u64,
    config_path: String,
    checks: Vec<DoctorCheck>,
    providers: Vec<DoctorProviderCheck>,
    summary: DoctorSummary,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    repairs: Vec<RepairOutcome>,
}

#[derive(Clone, Copy, Debug)]
struct DoctorCheckOptions {
    provider_timeout: Duration,
    codex_engine_timeout: Duration,
    check_providers: bool,
    probe_providers_live: bool,
    check_codex_engine: bool,
}

impl DoctorCheckOptions {
    fn for_run(quick: bool) -> Self {
        if quick {
            Self {
                provider_timeout: Duration::from_secs(2),
                codex_engine_timeout: Duration::from_secs(3),
                check_providers: true,
                probe_providers_live: false,
                check_codex_engine: false,
            }
        } else {
            Self {
                provider_timeout: Duration::from_secs(8),
                codex_engine_timeout: Duration::from_secs(15),
                check_providers: true,
                probe_providers_live: true,
                check_codex_engine: true,
            }
        }
    }

    fn for_repair_planning(self) -> Self {
        Self {
            check_providers: false,
            probe_providers_live: false,
            check_codex_engine: false,
            ..self
        }
    }
}

pub(super) async fn doctor(
    json_output: bool,
    fix: bool,
    restart_apps: bool,
    quick: bool,
) -> anyhow::Result<()> {
    let options = DoctorCheckOptions::for_run(quick);
    let mut repairs = Vec::new();
    if fix {
        // Provider discovery and app-server probing never produce repair actions.
        // Skip them while planning, then run them once after repairs for the final report.
        let planning_report = run_doctor_checks(options.for_repair_planning()).await?;
        let planned = planned_fixes(&planning_report.checks);
        let (skipped, applicable): (Vec<DoctorFix>, Vec<DoctorFix>) = planned
            .into_iter()
            .partition(|fix| fix.requires_restart_opt_in() && !restart_apps);
        if !skipped.is_empty() && !json_output {
            println!(
                "auto-fix: skipped {} app restart(s); they interrupt live sessions (pass --fix --restart-apps to apply)",
                skipped.len()
            );
        }
        if applicable.is_empty() {
            if !json_output {
                println!("auto-fix: nothing safe to repair");
            }
        } else {
            repairs = apply_doctor_fixes(&applicable).await;
        }
    }
    let mut report = run_doctor_checks(options).await?;
    report.repairs = repairs;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_doctor_report(&report, fix);
    }
    Ok(())
}

async fn run_doctor_checks(options: DoctorCheckOptions) -> anyhow::Result<DoctorReport> {
    let path = stored_config_path();
    let mut checks = Vec::new();
    let stored = match load_stored_config() {
        Ok(Some(config)) => {
            checks.push(
                DoctorCheck::new(
                    "stored_config",
                    "Provider config",
                    DoctorStatus::Ok,
                    format!("loaded {} provider(s)", config.providers.len()),
                )
                .detail(path.display().to_string()),
            );
            Some(config)
        }
        Ok(None) => {
            checks.push(
                DoctorCheck::new(
                    "stored_config",
                    "Provider config",
                    DoctorStatus::Error,
                    "config file is missing; add a provider first",
                )
                .detail(path.display().to_string()),
            );
            None
        }
        Err(error) => {
            checks.push(
                DoctorCheck::new(
                    "stored_config",
                    "Provider config",
                    DoctorStatus::Error,
                    "config file could not be read or parsed",
                )
                .detail(format!("{error:#}")),
            );
            None
        }
    };

    if path.exists() {
        checks.push(check_config_permissions(&path));
    }

    if stored.is_some() {
        match GatewayConfig::from_stored_config() {
            Ok(config) => checks.push(DoctorCheck::new(
                "runtime_config",
                "Runtime config",
                DoctorStatus::Ok,
                format!(
                    "{} provider(s), bind {}, runtime env overrides disabled",
                    config.providers.len(),
                    config.bind
                ),
            )),
            Err(error) => checks.push(
                DoctorCheck::new(
                    "runtime_config",
                    "Runtime config",
                    DoctorStatus::Error,
                    "config structure, provider routing, or fusion references failed validation",
                )
                .detail(format!("{error:#}")),
            ),
        }
    }

    let provider_task = if options.check_providers {
        stored.as_ref().map(|config| {
            let providers = config.providers.clone();
            tokio::spawn(check_doctor_providers(
                providers,
                options.provider_timeout,
                options.probe_providers_live,
            ))
        })
    } else {
        None
    };

    let (gateway_check, live) = check_gateway_runtime().await;
    checks.push(gateway_check);

    let gateway_api_key = stored
        .as_ref()
        .and_then(|config| config.gateway_api_key.clone());
    let mut gateway_model_ids = None;
    if let Some(live) = &live {
        let (check, ids) = check_gateway_models(live.bind, gateway_api_key.as_deref()).await;
        checks.push(check);
        gateway_model_ids = ids;
    }

    let (integration_checks, managed) = check_codex_integration(
        live.as_ref(),
        gateway_model_ids.as_deref(),
        gateway_api_key.as_deref(),
    );
    checks.extend(integration_checks);

    if let Some(managed) = &managed {
        if options.check_codex_engine {
            checks.push(check_codex_engine(managed, options.codex_engine_timeout).await);
        }
        #[cfg(target_os = "macos")]
        checks.extend(check_desktop_apps(&managed.config_path));
    }

    checks.push(check_gateway_log());
    let providers = match provider_task {
        Some(task) => task.await??,
        None => Vec::new(),
    };

    let ok = checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Ok)
        .count()
        + providers
            .iter()
            .filter(|check| check.status == DoctorStatus::Ok)
            .count();
    let warnings = checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Warning)
        .count()
        + providers
            .iter()
            .filter(|check| check.status == DoctorStatus::Warning)
            .count();
    let errors = checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Error)
        .count()
        + providers
            .iter()
            .filter(|check| check.status == DoctorStatus::Error)
            .count();
    Ok(DoctorReport {
        ok: errors == 0,
        generated_at_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
        config_path: path.display().to_string(),
        checks,
        providers,
        summary: DoctorSummary {
            ok,
            warnings,
            errors,
        },
        repairs: Vec::new(),
    })
}

async fn check_doctor_providers(
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

fn planned_fixes(checks: &[DoctorCheck]) -> Vec<DoctorFix> {
    let mut fixes: Vec<DoctorFix> = checks
        .iter()
        .flat_map(|check| check.auto_fixes.iter().copied())
        .collect();
    fixes.sort();
    fixes.dedup();
    fixes
}

async fn apply_doctor_fixes(fixes: &[DoctorFix]) -> Vec<RepairOutcome> {
    let mut outcomes = Vec::new();
    for &fix in fixes {
        println!("auto-fix: {} ...", fix.description());
        let result = apply_doctor_fix(fix).await;
        let (ok, message) = match result {
            Ok(message) => (true, message),
            Err(error) => (false, format!("{error:#}")),
        };
        println!(
            "auto-fix: {} => {}",
            fix.description(),
            if ok { &message } else { "failed" }
        );
        outcomes.push(RepairOutcome {
            fix,
            description: fix.description().to_owned(),
            ok,
            message,
        });
    }
    outcomes
}

async fn apply_doctor_fix(fix: DoctorFix) -> anyhow::Result<String> {
    match fix {
        DoctorFix::CleanStaleGatewayMetadata => {
            let mut removed = Vec::new();
            if let Some(runtime) = load_runtime_metadata()?
                && !pid_is_running(runtime.pid)?
            {
                delete_runtime_metadata()?;
                removed.push("runtime.json");
            }
            if let Some(daemon) = load_daemon_metadata()?
                && !pid_is_running(daemon.pid)?
            {
                delete_daemon_metadata()?;
                removed.push("daemon.json");
            }
            if removed.is_empty() {
                Ok("no stale runtime metadata to remove".to_owned())
            } else {
                Ok(format!("removed {}", removed.join(", ")))
            }
        }
        DoctorFix::StartGateway => {
            let config = GatewayConfig::from_stored_config()?;
            tokio::task::spawn_blocking(move || start_daemon(None, None, &config, false)).await??;
            let bind = load_runtime_metadata()?
                .map(|runtime| runtime.bind.to_string())
                .unwrap_or_else(|| "unknown".to_owned());
            Ok(format!("gateway started on {bind}"))
        }
        DoctorFix::FixConfigPermissions => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let path = stored_config_path();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                Ok(format!("chmod 600 applied to {}", path.display()))
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!("automatic permission repair is not supported on this platform")
            }
        }
        DoctorFix::SyncGatewayBaseUrl => {
            let config_path = resolve_codex_config_path(None)?;
            let runtime = load_runtime_metadata()?
                .filter(|runtime| pid_is_running(runtime.pid).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("gateway is not running; cannot sync base_url"))?;
            let changed = sync_managed_codex_gateway_base_url(&config_path, runtime.bind)?;
            Ok(if changed {
                format!("base_url updated to http://{}/v1", runtime.bind)
            } else {
                "base_url is already current".to_owned()
            })
        }
        DoctorFix::RefreshCodexCatalog => {
            refresh_default_managed_codex_catalog().await?;
            Ok("managed model catalog refreshed".to_owned())
        }
        DoctorFix::RestartChatGptApp => restart_app_fix("ChatGPT").await,
        DoctorFix::RestartCodexApp => restart_app_fix("Codex").await,
    }
}

async fn restart_app_fix(app: &'static str) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || restart_macos_app(app)).await?
}

#[cfg(target_os = "macos")]
fn restart_macos_app(app: &str) -> anyhow::Result<String> {
    let previous_pid = desktop_app_pid(app);
    let quit = ProcessCommand::new("osascript")
        .args(["-e", &format!("quit app \"{app}\"")])
        .status()?;
    if !quit.success() {
        anyhow::bail!("failed to quit {app}");
    }
    match previous_pid {
        Some(pid) => {
            let deadline = Instant::now() + Duration::from_secs(15);
            while pid_is_running(pid).unwrap_or(false) {
                if Instant::now() >= deadline {
                    anyhow::bail!(
                        "{app} did not exit within 15s (a window may be blocking quit); restart it manually"
                    );
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        None => std::thread::sleep(Duration::from_secs(2)),
    }
    let open = ProcessCommand::new("open").args(["-a", app]).status()?;
    if !open.success() {
        anyhow::bail!("failed to reopen {app}");
    }
    Ok(format!("{app} restarted"))
}

#[cfg(not(target_os = "macos"))]
fn restart_macos_app(app: &str) -> anyhow::Result<String> {
    anyhow::bail!("automatic app restart is not supported on this platform for {app}")
}

fn check_config_permissions(path: &Path) -> DoctorCheck {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(path) {
            Ok(metadata) if metadata.permissions().mode() & 0o077 == 0 => DoctorCheck::new(
                "config_permissions",
                "Config permissions",
                DoctorStatus::Ok,
                format!("{:o}", metadata.permissions().mode() & 0o777),
            ),
            Ok(metadata) => DoctorCheck::new(
                "config_permissions",
                "Config permissions",
                DoctorStatus::Warning,
                "config file is readable by accounts other than the current user",
            )
            .detail(format!(
                "current mode {:o}; recommend chmod 600 {}",
                metadata.permissions().mode() & 0o777,
                path.display()
            ))
            .fix(DoctorFix::FixConfigPermissions),
            Err(error) => DoctorCheck::new(
                "config_permissions",
                "Config permissions",
                DoctorStatus::Error,
                "could not read config file permissions",
            )
            .detail(error.to_string()),
        }
    }
    #[cfg(not(unix))]
    {
        DoctorCheck::new(
            "config_permissions",
            "Config permissions",
            DoctorStatus::Ok,
            "this platform does not use Unix permission checks",
        )
    }
}

async fn check_doctor_provider(
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

struct LiveGateway {
    bind: SocketAddr,
}

async fn check_gateway_runtime() -> (DoctorCheck, Option<LiveGateway>) {
    let runtime = match load_runtime_metadata() {
        Ok(Some(runtime)) => runtime,
        Ok(None) => {
            return (
                DoctorCheck::new(
                    "gateway",
                    "Local gateway",
                    DoctorStatus::Warning,
                    "gateway is not running",
                )
                .hint("codex-mixin service start")
                .fix(DoctorFix::StartGateway),
                None,
            );
        }
        Err(error) => {
            return (
                DoctorCheck::new(
                    "gateway",
                    "Local gateway",
                    DoctorStatus::Error,
                    "runtime metadata could not be read",
                )
                .detail(format!("{error:#}")),
                None,
            );
        }
    };
    match pid_is_running(runtime.pid) {
        Ok(false) => (
            DoctorCheck::new(
                "gateway",
                "Local gateway",
                DoctorStatus::Warning,
                format!(
                    "stale runtime metadata found: pid {} is not running",
                    runtime.pid
                ),
            )
            .detail(format!("bind {}", runtime.bind))
            .fix(DoctorFix::CleanStaleGatewayMetadata)
            .fix(DoctorFix::StartGateway),
            None,
        ),
        Err(error) => (
            DoctorCheck::new(
                "gateway",
                "Local gateway",
                DoctorStatus::Error,
                format!("could not inspect gateway pid {}", runtime.pid),
            )
            .detail(format!("{error:#}")),
            None,
        ),
        Ok(true) => {
            let url = format!("http://{}/healthz", runtime.bind);
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build();
            let client = match client {
                Ok(client) => client,
                Err(error) => {
                    return (
                        DoctorCheck::new(
                            "gateway",
                            "Local gateway",
                            DoctorStatus::Error,
                            "could not build the local gateway health request",
                        )
                        .detail(format!("{error:#}")),
                        None,
                    );
                }
            };
            match client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    let current_version = env!("CARGO_PKG_VERSION");
                    let live = Some(LiveGateway { bind: runtime.bind });
                    match runtime.version.as_deref() {
                        Some(version) if version != current_version => (
                            DoctorCheck::new(
                                "gateway",
                                "Local gateway",
                                DoctorStatus::Warning,
                                format!(
                                    "running as pid {} on {}, but gateway version {} does not match CLI version {}",
                                    runtime.pid, runtime.bind, version, current_version
                                ),
                            )
                            .hint("codex-mixin restart")
                            .fix(DoctorFix::StartGateway),
                            live,
                        ),
                        version => {
                            let mut check = DoctorCheck::new(
                                "gateway",
                                "Local gateway",
                                DoctorStatus::Ok,
                                format!("running as pid {} on {}", runtime.pid, runtime.bind),
                            );
                            if let Some(version) = version {
                                check = check.detail(format!("version {version}"));
                            }
                            (check, live)
                        }
                    }
                }
                Ok(response) => (
                    DoctorCheck::new(
                        "gateway",
                        "Local gateway",
                        DoctorStatus::Error,
                        format!("healthz returned {}", response.status()),
                    )
                    .detail(url)
                    .hint("codex-mixin restart"),
                    None,
                ),
                Err(error) => (
                    DoctorCheck::new(
                        "gateway",
                        "Local gateway",
                        DoctorStatus::Error,
                        "process is alive but healthz is unreachable",
                    )
                    .detail(format!("{error:#}"))
                    .hint("codex-mixin restart"),
                    None,
                ),
            }
        }
    }
}

async fn check_gateway_models(
    bind: SocketAddr,
    gateway_api_key: Option<&str>,
) -> (DoctorCheck, Option<Vec<String>>) {
    let url = format!("http://{bind}/v1/models");
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return (
                DoctorCheck::new(
                    "gateway_models",
                    "Gateway models",
                    DoctorStatus::Error,
                    "could not build the models list request",
                )
                .detail(format!("{error:#}")),
                None,
            );
        }
    };
    let mut request = client.get(&url);
    if let Some(key) = gateway_api_key {
        request = request.bearer_auth(key);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => {
            let body = match response.json::<serde_json::Value>().await {
                Ok(body) => body,
                Err(error) => {
                    return (
                        DoctorCheck::new(
                            "gateway_models",
                            "Gateway models",
                            DoctorStatus::Error,
                            "models list response is not valid JSON",
                        )
                        .detail(format!("{error:#}")),
                        None,
                    );
                }
            };
            let ids: Vec<String> = body
                .get("data")
                .and_then(serde_json::Value::as_array)
                .map(|models| {
                    models
                        .iter()
                        .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if ids.is_empty() {
                (
                    DoctorCheck::new(
                        "gateway_models",
                        "Gateway models",
                        DoctorStatus::Error,
                        "gateway /v1/models returned an empty list; Codex will have no models",
                    )
                    .detail(url),
                    Some(ids),
                )
            } else {
                (
                    DoctorCheck::new(
                        "gateway_models",
                        "Gateway models",
                        DoctorStatus::Ok,
                        format!("gateway /v1/models returned {} model(s)", ids.len()),
                    )
                    .detail(url),
                    Some(ids),
                )
            }
        }
        Ok(response)
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                || response.status() == reqwest::StatusCode::FORBIDDEN =>
        {
            (
                DoctorCheck::new(
                    "gateway_models",
                    "Gateway models",
                    DoctorStatus::Error,
                    format!(
                        "gateway rejected the models list request ({}); API key validation failed",
                        response.status()
                    ),
                )
                .detail(url),
                None,
            )
        }
        Ok(response) => (
            DoctorCheck::new(
                "gateway_models",
                "Gateway models",
                DoctorStatus::Error,
                format!("gateway /v1/models returned {}", response.status()),
            )
            .detail(url),
            None,
        ),
        Err(error) => (
            DoctorCheck::new(
                "gateway_models",
                "Gateway models",
                DoctorStatus::Error,
                "could not request the gateway models list",
            )
            .detail(format!("{url}: {error:#}")),
            None,
        ),
    }
}

struct ManagedIntegration {
    config_path: PathBuf,
    codex_home: PathBuf,
    managed_slugs: HashSet<String>,
}

fn normalized_model_key(raw: &str) -> String {
    raw.strip_suffix("-custom").unwrap_or(raw).to_owned()
}

fn check_codex_integration(
    live: Option<&LiveGateway>,
    gateway_model_ids: Option<&[String]>,
    gateway_api_key: Option<&str>,
) -> (Vec<DoctorCheck>, Option<ManagedIntegration>) {
    let mut checks = Vec::new();
    let path = match resolve_codex_config_path(None) {
        Ok(path) => path,
        Err(error) => {
            checks.push(
                DoctorCheck::new(
                    "codex_config",
                    "Codex integration",
                    DoctorStatus::Error,
                    "could not resolve the Codex config path",
                )
                .detail(format!("{error:#}")),
            );
            return (checks, None);
        }
    };
    if !path.exists() {
        checks.push(
            DoctorCheck::new(
                "codex_config",
                "Codex integration",
                DoctorStatus::Warning,
                "Codex config file is missing; codex-mixin is not installed into Codex yet",
            )
            .detail(path.display().to_string())
            .hint("codex-mixin connect codex --custom-only"),
        );
        return (checks, None);
    }
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) => {
            checks.push(
                DoctorCheck::new(
                    "codex_config",
                    "Codex integration",
                    DoctorStatus::Error,
                    "Codex config file could not be read",
                )
                .detail(format!("{}: {error}", path.display())),
            );
            return (checks, None);
        }
    };
    if !is_managed_config(&raw) {
        checks.push(
            DoctorCheck::new(
                "codex_config",
                "Codex integration",
                DoctorStatus::Warning,
                "Codex is not using a codex-mixin managed config",
            )
            .detail(path.display().to_string())
            .hint("codex-mixin connect codex --custom-only"),
        );
        return (checks, None);
    }
    checks.push(
        DoctorCheck::new(
            "codex_config",
            "Codex integration",
            DoctorStatus::Ok,
            "Codex is currently managed by codex-mixin",
        )
        .detail(path.display().to_string()),
    );

    let doc = match raw.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(error) => {
            checks.push(
                DoctorCheck::new(
                    "codex_managed_config",
                    "Managed Codex config",
                    DoctorStatus::Error,
                    "managed config could not be parsed as TOML",
                )
                .detail(format!("{error:#}"))
                .hint("rerun codex-mixin connect codex"),
            );
            return (checks, None);
        }
    };

    let effective_provider = doc.get("model_provider").and_then(Item::as_str);
    let managed_provider = managed_config_provider_id(&doc).ok();
    if managed_provider.is_none() {
        checks.push(
            DoctorCheck::new(
                "codex_model_provider",
                "Codex default provider",
                DoctorStatus::Error,
                format!(
                    "model_provider {:?} is not a codex-mixin managed provider",
                    effective_provider.unwrap_or("<unset>")
                ),
            )
            .hint("rerun codex-mixin connect codex"),
        );
    }

    let provider_table = doc
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| managed_provider.and_then(|id| providers.get(id)))
        .and_then(Item::as_table);
    let oauth_proxy = managed_provider == Some(CODEX_MIXIN_PROVIDER)
        && provider_table
            .and_then(|table| table.get("supports_websockets"))
            .and_then(Item::as_bool)
            .unwrap_or(false);

    match (
        provider_table
            .and_then(|table| table.get("base_url"))
            .and_then(Item::as_str),
        live,
    ) {
        (Some(base_url), Some(live)) => {
            let expected = format!("http://{}/v1", live.bind);
            if base_url == expected {
                checks.push(DoctorCheck::new(
                    "codex_base_url",
                    "Codex gateway URL",
                    DoctorStatus::Ok,
                    format!("base_url points at the current gateway {expected}"),
                ));
            } else {
                checks.push(
                    DoctorCheck::new(
                        "codex_base_url",
                        "Codex gateway URL",
                        DoctorStatus::Error,
                        format!(
                            "base_url {base_url} does not match the running gateway {expected}; Codex will not reach the gateway"
                        ),
                    )
                    .fix(DoctorFix::SyncGatewayBaseUrl),
                );
            }
        }
        (Some(base_url), None) => {
            checks.push(
                DoctorCheck::new(
                    "codex_base_url",
                    "Codex gateway URL",
                    DoctorStatus::Warning,
                    "gateway is not running; cannot verify whether base_url is reachable",
                )
                .detail(format!("configured base_url: {base_url}")),
            );
        }
        (None, _) => {
            checks.push(
                DoctorCheck::new(
                    "codex_base_url",
                    "Codex gateway URL",
                    DoctorStatus::Error,
                    "managed config is missing base_url for the current provider",
                )
                .hint("rerun codex-mixin connect codex"),
            );
        }
    }

    if !oauth_proxy {
        let env_key = provider_table
            .and_then(|table| table.get("env_key"))
            .and_then(Item::as_str);
        match (env_key, gateway_api_key) {
            (Some(key), _) => {
                if std::env::var(key).is_err() {
                    checks.push(
                        DoctorCheck::new(
                            "codex_env_key",
                            "Codex API key environment variable",
                            DoctorStatus::Warning,
                            format!("env_key={key} is configured, but the variable is not set in the current environment"),
                        )
                        .detail(
                            "Codex Desktop environment variables need launchctl setenv; ignore this if gateway auth is disabled",
                        ),
                    );
                } else {
                    checks.push(DoctorCheck::new(
                        "codex_env_key",
                        "Codex API key environment variable",
                        DoctorStatus::Ok,
                        format!("env_key={key} is available in the current environment"),
                    ));
                }
            }
            (None, Some(_)) => {
                checks.push(
                    DoctorCheck::new(
                        "codex_env_key",
                        "Codex API key environment variable",
                        DoctorStatus::Warning,
                        "gateway auth is enabled, but Codex config has no env_key; Codex requests will be rejected",
                    )
                    .hint("rerun codex-mixin connect codex"),
                );
            }
            (None, None) => {}
        }
    }

    let codex_home = match path.parent() {
        Some(parent) => parent.to_path_buf(),
        None => {
            checks.push(DoctorCheck::new(
                "codex_catalog",
                "Codex model catalog",
                DoctorStatus::Error,
                "Codex config path has no parent directory",
            ));
            return (checks, None);
        }
    };

    if oauth_proxy {
        let models_cache = codex_home.join("models_cache.json");
        if !models_cache.exists() {
            checks.push(
                DoctorCheck::new(
                    "codex_models_cache",
                    "Official model cache",
                    DoctorStatus::Error,
                    "codex_oauth_proxy mode requires models_cache.json, but the file is missing",
                )
                .detail(models_cache.display().to_string())
                .hint("sign in with the official account, open Codex once to create the cache, then reinstall"),
            );
        }
    }

    let catalog_path = match managed_catalog_path(&doc, &path) {
        Ok(catalog_path) => catalog_path,
        Err(error) => {
            checks.push(
                DoctorCheck::new(
                    "codex_catalog",
                    "Codex model catalog",
                    DoctorStatus::Error,
                    "managed config is missing model_catalog_json",
                )
                .detail(format!("{error:#}"))
                .hint("rerun codex-mixin connect codex"),
            );
            return (checks, None);
        }
    };
    let catalog_raw = match fs::read(&catalog_path) {
        Ok(raw) => raw,
        Err(error) => {
            checks.push(
                DoctorCheck::new(
                    "codex_catalog",
                    "Codex model catalog",
                    DoctorStatus::Error,
                    "managed model catalog file could not be read",
                )
                .detail(format!("{}: {error}", catalog_path.display()))
                .fix(DoctorFix::RefreshCodexCatalog),
            );
            return (
                checks,
                Some(ManagedIntegration {
                    config_path: path,
                    codex_home,
                    managed_slugs: HashSet::new(),
                }),
            );
        }
    };
    let catalog_models = serde_json::from_slice::<serde_json::Value>(&catalog_raw)
        .ok()
        .and_then(|catalog| {
            catalog
                .get("models")
                .and_then(serde_json::Value::as_array)
                .cloned()
        });
    let Some(catalog_models) = catalog_models else {
        checks.push(
            DoctorCheck::new(
                "codex_catalog",
                "Codex model catalog",
                DoctorStatus::Error,
                "managed model catalog is not valid catalog JSON",
            )
            .detail(catalog_path.display().to_string())
            .fix(DoctorFix::RefreshCodexCatalog),
        );
        return (
            checks,
            Some(ManagedIntegration {
                config_path: path,
                codex_home,
                managed_slugs: HashSet::new(),
            }),
        );
    };

    let all_slugs: HashSet<String> = catalog_models
        .iter()
        .filter_map(|model| model.get("slug").and_then(serde_json::Value::as_str))
        .map(normalized_model_key)
        .collect();
    let mut managed_slugs: HashSet<String> = catalog_models
        .iter()
        .filter(|model| is_managed_catalog_model(model))
        .filter_map(|model| model.get("slug").and_then(serde_json::Value::as_str))
        .map(normalized_model_key)
        .collect();
    if !oauth_proxy && managed_slugs.is_empty() {
        managed_slugs = all_slugs.clone();
    }

    if catalog_models.is_empty() {
        checks.push(
            DoctorCheck::new(
                "codex_catalog",
                "Codex model catalog",
                DoctorStatus::Error,
                "managed model catalog is empty; the Codex model picker will have no models",
            )
            .detail(catalog_path.display().to_string())
            .fix(DoctorFix::RefreshCodexCatalog),
        );
    } else {
        checks.push(
            DoctorCheck::new(
                "codex_catalog",
                "Codex model catalog",
                DoctorStatus::Ok,
                format!(
                    "catalog contains {} model(s), {} of which are managed by codex-mixin",
                    catalog_models.len(),
                    managed_slugs.len()
                ),
            )
            .detail(catalog_path.display().to_string()),
        );
    }

    if !oauth_proxy && let Some(ids) = gateway_model_ids {
        let expected: HashSet<String> = ids.iter().map(|id| normalized_model_key(id)).collect();
        let missing: Vec<&String> = expected.difference(&all_slugs).collect();
        let extra: Vec<&String> = all_slugs.difference(&expected).collect();
        if missing.is_empty() && extra.is_empty() {
            checks.push(DoctorCheck::new(
                "codex_catalog_sync",
                "Model catalog sync",
                DoctorStatus::Ok,
                "model catalog matches the current gateway models",
            ));
        } else {
            let mut detail = Vec::new();
            if !missing.is_empty() {
                detail.push(format!(
                    "catalog is missing {} gateway model(s): {}",
                    missing.len(),
                    preview_list(&missing)
                ));
            }
            if !extra.is_empty() {
                detail.push(format!(
                    "catalog contains {} model(s) no longer present on the gateway: {}",
                    extra.len(),
                    preview_list(&extra)
                ));
            }
            checks.push(
                DoctorCheck::new(
                    "codex_catalog_sync",
                    "Model catalog sync",
                    DoctorStatus::Warning,
                    "model catalog does not match the current gateway models",
                )
                .detail(detail.join("；"))
                .fix(DoctorFix::RefreshCodexCatalog),
            );
        }
    }

    (
        checks,
        Some(ManagedIntegration {
            config_path: path,
            codex_home,
            managed_slugs,
        }),
    )
}

fn preview_list(items: &[&String]) -> String {
    let mut preview: Vec<&str> = items.iter().take(5).map(|item| item.as_str()).collect();
    preview.sort_unstable();
    let mut text = preview.join(", ");
    if items.len() > 5 {
        text.push_str(", ...");
    }
    text
}

#[derive(Debug, Default)]
struct AppServerProbe {
    initialize_ok: bool,
    user_agent: Option<String>,
    reported_codex_home: Option<String>,
    model_list_ok: bool,
    model_ids: Vec<String>,
}

/// Open an `app-server` session against the Codex engine and call `model/list`
/// so doctor verifies what the Desktop picker actually sees, not only the catalog file.
async fn check_codex_engine(managed: &ManagedIntegration, timeout: Duration) -> DoctorCheck {
    let cli = match resolve_codex_cli() {
        Ok(cli) => cli,
        Err(error) => {
            return DoctorCheck::new(
                "codex_engine",
                "Codex engine probe",
                DoctorStatus::Warning,
                "Codex CLI was not found; skipped engine model/list probe",
            )
            .detail(format!("{error:#}"))
            .hint("set CODEX_CLI_PATH or install the Codex/ChatGPT app");
        }
    };
    let codex_home = managed.codex_home.clone();
    let cli_for_probe = cli.clone();
    let probe = tokio::task::spawn_blocking(move || {
        probe_codex_app_server(&cli_for_probe, &codex_home, timeout)
    })
    .await;
    let probe = match probe {
        Ok(Ok(probe)) => probe,
        Ok(Err(error)) => {
            return DoctorCheck::new(
                "codex_engine",
                "Codex engine probe",
                DoctorStatus::Warning,
                "could not start codex app-server for the engine probe",
            )
            .detail(format!("{}: {error:#}", cli.display()));
        }
        Err(error) => {
            return DoctorCheck::new(
                "codex_engine",
                "Codex engine probe",
                DoctorStatus::Warning,
                "engine probe task failed",
            )
            .detail(format!("{error:#}"));
        }
    };
    let engine = probe
        .user_agent
        .clone()
        .unwrap_or_else(|| "unknown".to_owned());
    if !probe.initialize_ok {
        return DoctorCheck::new(
            "codex_engine",
            "Codex engine probe",
            DoctorStatus::Warning,
            "Codex engine did not answer initialize; cannot confirm picker-visible models",
        )
        .detail(format!("cli: {}", cli.display()));
    }
    if let Some(reported) = &probe.reported_codex_home {
        let expected = canonical_display(&managed.codex_home);
        let reported_canonical = canonical_display(Path::new(reported));
        if expected != reported_canonical {
            return DoctorCheck::new(
                "codex_engine",
                "Codex engine probe",
                DoctorStatus::Error,
                "Codex engine CODEX_HOME does not match the install target",
            )
            .detail(format!(
                "engine: {reported_canonical}; install target: {expected}; user-agent: {engine}"
            ))
            .hint("check the Desktop app CODEX_HOME environment variable");
        }
    }
    if !probe.model_list_ok {
        return DoctorCheck::new(
            "codex_engine",
            "Codex engine probe",
            DoctorStatus::Warning,
            "Codex engine does not support model/list or timed out; cannot confirm picker-visible models",
        )
        .detail(format!("engine: {engine}"));
    }
    let expected = managed.managed_slugs.len();
    let visible: HashSet<String> = probe
        .model_ids
        .iter()
        .map(|id| normalized_model_key(id))
        .filter(|key| managed.managed_slugs.contains(key))
        .collect();
    if visible.is_empty() && expected > 0 {
        return DoctorCheck::new(
            "codex_engine",
            "Codex engine probe",
            DoctorStatus::Error,
            format!(
                "engine model/list returned {} model(s), but none are managed; the Desktop picker will be empty",
                probe.model_ids.len()
            ),
        )
        .detail(format!("engine: {engine}"))
        .hint("confirm Desktop and CLI share the same CODEX_HOME, then fully quit and reopen the app");
    }
    if visible.len() < expected {
        let missing: Vec<&String> = managed
            .managed_slugs
            .iter()
            .filter(|slug| {
                !probe
                    .model_ids
                    .iter()
                    .any(|id| normalized_model_key(id) == **slug)
            })
            .collect();
        return DoctorCheck::new(
            "codex_engine",
            "Codex engine probe",
            DoctorStatus::Warning,
            format!(
                "engine can see only {}/{expected} managed model(s)",
                visible.len()
            ),
        )
        .detail(format!(
            "missing: {}; engine: {engine}",
            preview_list(&missing)
        ));
    }
    DoctorCheck::new(
        "codex_engine",
        "Codex engine probe",
        DoctorStatus::Ok,
        format!(
            "engine probe can see {}/{expected} managed model(s) (app-server model/list returned {} model(s))",
            visible.len(),
            probe.model_ids.len()
        ),
    )
    .detail(format!("engine: {engine}"))
}

fn canonical_display(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn probe_codex_app_server(
    cli: &Path,
    codex_home: &Path,
    timeout: Duration,
) -> anyhow::Result<AppServerProbe> {
    let mut child = ProcessCommand::new(cli)
        .arg("app-server")
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let pid = child.id();
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        if cancel_rx.recv_timeout(timeout).is_err() {
            let _ = ProcessCommand::new("kill")
                .args(["-9", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    });
    let result = probe_app_server_streams(&mut child);
    let _ = cancel_tx.send(());
    let _ = child.kill();
    let _ = child.wait();
    let _ = watchdog.join();
    result
}

fn probe_app_server_streams(child: &mut std::process::Child) -> anyhow::Result<AppServerProbe> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("app-server stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("app-server stdout unavailable"))?;
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {"name": "codex-mixin-doctor", "version": env!("CARGO_PKG_VERSION")}}
        })
    )?;
    stdin.flush()?;
    let mut probe = AppServerProbe::default();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match value.get("id").and_then(serde_json::Value::as_u64) {
            Some(1) => {
                if let Some(result) = value.get("result") {
                    probe.initialize_ok = true;
                    probe.user_agent = result.get("userAgent").map(|agent| match agent {
                        serde_json::Value::String(agent) => agent.clone(),
                        other => other.to_string(),
                    });
                    probe.reported_codex_home = result
                        .get("codexHome")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                } else {
                    break;
                }
                writeln!(stdin, "{}", serde_json::json!({"method": "initialized"}))?;
                writeln!(
                    stdin,
                    "{}",
                    serde_json::json!({
                        "id": 2,
                        "method": "model/list",
                        "params": {"limit": 1000, "includeHidden": false}
                    })
                )?;
                stdin.flush()?;
            }
            Some(2) => {
                if let Some(data) = value
                    .pointer("/result/data")
                    .and_then(serde_json::Value::as_array)
                {
                    probe.model_list_ok = true;
                    probe.model_ids = data
                        .iter()
                        .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
                        .map(str::to_owned)
                        .collect();
                }
                break;
            }
            _ => {}
        }
    }
    Ok(probe)
}

#[cfg(target_os = "macos")]
fn check_desktop_apps(managed_config_path: &Path) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let config_mtime = fs::metadata(managed_config_path)
        .and_then(|metadata| metadata.modified())
        .ok();
    let mut any_running = false;
    for (app, fix) in [
        ("ChatGPT", DoctorFix::RestartChatGptApp),
        ("Codex", DoctorFix::RestartCodexApp),
    ] {
        let Some(pid) = desktop_app_pid(app) else {
            continue;
        };
        any_running = true;
        let id = format!("desktop_app_{}", app.to_lowercase());
        let name = format!("{app} App");
        match (process_start_time(pid), config_mtime) {
            (Some(started), Some(mtime)) if started < mtime => {
                checks.push(
                    DoctorCheck::new(
                        id,
                        name,
                        DoctorStatus::Warning,
                        format!("{app} started before the config update and is still using the old config; restart required"),
                    )
                    .detail(format!("pid {pid}; restart to load the latest model catalog"))
                    .hint(format!(
                        "osascript -e 'quit app \"{app}\"' && sleep 2 && open -a {app}"
                    ))
                    .fix(fix),
                );
            }
            (Some(_), _) => {
                checks.push(DoctorCheck::new(
                    id,
                    name,
                    DoctorStatus::Ok,
                    format!("{app} started after the config update and is using the latest config"),
                ));
            }
            (None, _) => {
                checks.push(
                    DoctorCheck::new(
                        id,
                        name,
                        DoctorStatus::Warning,
                        format!("{app} is running, but its start time could not be determined"),
                    )
                    .detail(format!("PID {pid}")),
                );
            }
        }
    }
    if !any_running {
        checks.push(DoctorCheck::new(
            "desktop_app",
            "Desktop App",
            DoctorStatus::Ok,
            "no running ChatGPT/Codex app detected; the next launch will load the latest config",
        ));
    }
    checks
}

/// Finds the main process of a desktop app bundle by matching the executable
/// path suffix, which is more reliable than `pgrep -x` for .app bundles.
#[cfg(target_os = "macos")]
fn desktop_app_pid(app: &str) -> Option<u32> {
    let output = ProcessCommand::new("ps")
        .args(["-axo", "pid=,comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let suffix = format!("/{app}.app/Contents/MacOS/{app}");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let (pid, command) = line.trim_start().split_once(' ')?;
            command
                .trim()
                .ends_with(&suffix)
                .then(|| pid.parse().ok())
                .flatten()
        })
}

#[cfg(target_os = "macos")]
fn process_start_time(pid: u32) -> Option<SystemTime> {
    let output = ProcessCommand::new("ps")
        .args(["-p", &pid.to_string(), "-o", "etime="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let elapsed = parse_ps_etime(String::from_utf8_lossy(&output.stdout).trim())?;
    SystemTime::now().checked_sub(elapsed)
}

/// Parses `ps -o etime=` values shaped like `[[dd-]hh:]mm:ss`.
fn parse_ps_etime(raw: &str) -> Option<Duration> {
    let (days, rest) = match raw.split_once('-') {
        Some((days, rest)) => (days.parse::<u64>().ok()?, rest),
        None => (0, raw),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let (hours, minutes, seconds): (u64, u64, u64) = match parts.as_slice() {
        [hours, minutes, seconds] => (
            hours.parse().ok()?,
            minutes.parse().ok()?,
            seconds.parse().ok()?,
        ),
        [minutes, seconds] => (0, minutes.parse().ok()?, seconds.parse().ok()?),
        _ => return None,
    };
    Some(Duration::from_secs(
        ((days * 24 + hours) * 60 + minutes) * 60 + seconds,
    ))
}

fn check_gateway_log() -> DoctorCheck {
    let path = default_log_file_path();
    match fs::metadata(&path) {
        Ok(metadata) if metadata.len() > 0 => {
            let (error_count, last_error) = scan_recent_log_errors(&path);
            if error_count > 0 {
                let mut check = DoctorCheck::new(
                    "gateway_log",
                    "Runtime log",
                    DoctorStatus::Warning,
                    format!(
                        "log available, {} bytes; this gateway run recorded {error_count} error line(s)",
                        metadata.len()
                    ),
                )
                .hint("run codex-mixin logs -n 200 for full context");
                if let Some(last_error) = last_error {
                    check = check.detail(format!("latest error: {last_error}"));
                }
                check
            } else {
                DoctorCheck::new(
                    "gateway_log",
                    "Runtime log",
                    DoctorStatus::Ok,
                    format!("log available, {} bytes", metadata.len()),
                )
                .detail(path.display().to_string())
            }
        }
        Ok(_) => DoctorCheck::new(
            "gateway_log",
            "Runtime log",
            DoctorStatus::Warning,
            "log file is empty",
        )
        .detail(path.display().to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DoctorCheck::new(
            "gateway_log",
            "Runtime log",
            DoctorStatus::Warning,
            "log file has not been created yet",
        )
        .detail(path.display().to_string()),
        Err(error) => DoctorCheck::new(
            "gateway_log",
            "Runtime log",
            DoctorStatus::Error,
            "log file could not be read",
        )
        .detail(format!("{}: {error}", path.display())),
    }
}

const LOG_SCAN_TAIL_BYTES: u64 = 64 * 1024;
const GATEWAY_START_MARKER: &str = "gateway process starting";

/// Counts ERROR lines that belong to the current gateway run (after the most
/// recent startup marker within the tail of the log).
fn scan_recent_log_errors(path: &Path) -> (usize, Option<String>) {
    let Ok(mut file) = fs::File::open(path) else {
        return (0, None);
    };
    let Ok(metadata) = file.metadata() else {
        return (0, None);
    };
    let offset = metadata.len().saturating_sub(LOG_SCAN_TAIL_BYTES);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return (0, None);
    }
    let mut tail = Vec::new();
    if file.read_to_end(&mut tail).is_err() {
        return (0, None);
    }
    let tail = String::from_utf8_lossy(&tail);
    let current_run = tail
        .rfind(GATEWAY_START_MARKER)
        .map_or(tail.as_ref(), |index| &tail[index..]);
    count_error_lines(current_run)
}

fn count_error_lines(text: &str) -> (usize, Option<String>) {
    let mut count = 0;
    let mut last = None;
    for line in text.lines() {
        if line.contains(" ERROR ") {
            count += 1;
            last = Some(truncated(line, 240));
        }
    }
    (count, last)
}

fn truncated(raw: &str, max_chars: usize) -> String {
    if raw.chars().count() <= max_chars {
        return raw.trim().to_owned();
    }
    let mut text: String = raw.trim().chars().take(max_chars).collect();
    text.push_str("...");
    text
}

fn print_doctor_report(report: &DoctorReport, fix_mode: bool) {
    println!("{}", style("Codex Mixin health check").bold());
    println!("{} {}", style("config:").dim(), report.config_path);
    for check in &report.checks {
        println!(
            "{} {}: {}",
            check.status.icon(),
            style(&check.name).bold(),
            check.message
        );
        if let Some(detail) = &check.detail {
            println!("  {}", style(detail).dim());
        }
        if let Some(hint) = &check.fix_hint {
            println!("  {} {hint}", style("hint:").cyan());
        }
        if !check.auto_fixes.is_empty() {
            println!(
                "  {} {}",
                style("auto-fix:").cyan(),
                check
                    .auto_fixes
                    .iter()
                    .map(|fix| fix.description())
                    .collect::<Vec<_>>()
                    .join("；")
            );
        }
    }
    for provider in &report.providers {
        println!(
            "{} {} {}: {}",
            provider.status.icon(),
            style("Provider").dim(),
            style(&provider.provider_id).bold(),
            provider.message
        );
        if let Some(detail) = &provider.detail {
            println!("  {}", style(detail).dim());
        }
    }
    if !report.repairs.is_empty() {
        println!("{}", style("repairs:").bold());
        for repair in &report.repairs {
            let icon = if repair.ok {
                style("✓").green().bold()
            } else {
                style("✗").red().bold()
            };
            println!("{} {}: {}", icon, repair.description, repair.message);
        }
    }
    let summary_style = if report.summary.errors > 0 {
        style("summary:").red().bold()
    } else if report.summary.warnings > 0 {
        style("summary:").yellow().bold()
    } else {
        style("summary:").green().bold()
    };
    println!(
        "{} {} ok, {} warnings, {} errors",
        summary_style, report.summary.ok, report.summary.warnings, report.summary.errors
    );
    let available = planned_fixes(&report.checks);
    if !fix_mode && !available.is_empty() {
        let restart_count = available
            .iter()
            .filter(|fix| fix.requires_restart_opt_in())
            .count();
        let plain_count = available.len() - restart_count;
        if plain_count > 0 {
            println!(
                "{} run `codex-mixin doctor --fix` to repair {plain_count} item(s)",
                style("→").cyan()
            );
        }
        if restart_count > 0 {
            println!(
                "{} app restarts need explicit confirmation; run `codex-mixin doctor --fix --restart-apps` (this interrupts live sessions)",
                style("→").cyan()
            );
        }
    }
    if report.ok {
        println!("{} doctor: ok", style("✓").green().bold());
    } else {
        println!("{} doctor: issues found", style("✗").red().bold());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_mixin::provider::{ProviderModel, custom_provider};

    #[test]
    fn quick_doctor_caps_expensive_checks() {
        let quick = DoctorCheckOptions::for_run(true);
        assert_eq!(quick.provider_timeout, Duration::from_secs(2));
        assert_eq!(quick.codex_engine_timeout, Duration::from_secs(3));
        assert!(quick.check_providers);
        assert!(!quick.probe_providers_live);
        assert!(!quick.check_codex_engine);

        let planning = quick.for_repair_planning();
        assert!(!planning.check_providers);
        assert!(!planning.probe_providers_live);
        assert!(!planning.check_codex_engine);
    }

    #[tokio::test]
    async fn quick_provider_check_never_waits_for_upstream() {
        let mut provider = custom_provider("slow-upstream", "secret");
        provider.base_url = "http://127.0.0.1:9".to_owned();
        provider.selected_models = vec!["cached-model".to_owned()];
        provider.cached_models = vec![ProviderModel {
            id: "cached-model".to_owned(),
            ..ProviderModel::default()
        }];
        provider.models_refreshed_at_ms = Some(1);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        let started = Instant::now();
        let check = check_doctor_provider(client, provider, false).await;

        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(check.status, DoctorStatus::Ok);
        assert!(check.message.contains("without contacting upstream"));
    }

    #[test]
    fn parses_ps_etime_variants() {
        assert_eq!(parse_ps_etime("05:20"), Some(Duration::from_secs(320)));
        assert_eq!(parse_ps_etime("01:02:03"), Some(Duration::from_secs(3723)));
        assert_eq!(
            parse_ps_etime("2-01:02:03"),
            Some(Duration::from_secs(2 * 86_400 + 3723))
        );
        assert_eq!(parse_ps_etime(""), None);
        assert_eq!(parse_ps_etime("garbage"), None);
    }

    #[test]
    fn normalizes_custom_suffix() {
        assert_eq!(normalized_model_key("gpt-5.6-sol-custom"), "gpt-5.6-sol");
        assert_eq!(
            normalized_model_key("auto-baidu-oneapi"),
            "auto-baidu-oneapi"
        );
    }

    #[test]
    fn planned_fixes_are_ordered_and_deduplicated() {
        let checks = vec![
            DoctorCheck::new("a", "a", DoctorStatus::Warning, "a")
                .fix(DoctorFix::RefreshCodexCatalog)
                .fix(DoctorFix::StartGateway),
            DoctorCheck::new("b", "b", DoctorStatus::Warning, "b")
                .fix(DoctorFix::StartGateway)
                .fix(DoctorFix::CleanStaleGatewayMetadata),
        ];
        assert_eq!(
            planned_fixes(&checks),
            vec![
                DoctorFix::CleanStaleGatewayMetadata,
                DoctorFix::StartGateway,
                DoctorFix::RefreshCodexCatalog,
            ]
        );
    }

    #[test]
    fn counts_error_lines_in_current_run() {
        let log = "2026-07-24T01:00:00Z ERROR old failure\n\
                   2026-07-24T02:00:00Z INFO gateway process starting\n\
                   2026-07-24T02:00:01Z INFO ready\n\
                   2026-07-24T02:10:00Z ERROR upstream timed out\n";
        let current_run = log
            .rfind(GATEWAY_START_MARKER)
            .map_or(log, |index| &log[index..]);
        let (count, last) = count_error_lines(current_run);
        assert_eq!(count, 1);
        assert!(last.unwrap().contains("upstream timed out"));
    }

    #[test]
    fn interprets_app_server_model_list_lines() {
        let mut probe = AppServerProbe::default();
        let init: serde_json::Value = serde_json::from_str(
            r#"{"id":1,"result":{"userAgent":"codex/0.146.0","codexHome":"/tmp/home"}}"#,
        )
        .unwrap();
        probe.initialize_ok = init.get("result").is_some();
        let list: serde_json::Value = serde_json::from_str(
            r#"{"id":2,"result":{"data":[{"id":"auto-baidu-oneapi"},{"id":"gpt-5.6-sol-custom"}]}}"#,
        )
        .unwrap();
        let ids: Vec<String> = list
            .pointer("/result/data")
            .and_then(serde_json::Value::as_array)
            .unwrap()
            .iter()
            .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
            .map(str::to_owned)
            .collect();
        probe.model_ids = ids;
        let slugs: HashSet<String> = ["auto-baidu-oneapi", "gpt-5.6-sol"]
            .iter()
            .map(|slug| (*slug).to_owned())
            .collect();
        let visible = probe
            .model_ids
            .iter()
            .filter(|id| slugs.contains(&normalized_model_key(id)))
            .count();
        assert_eq!(visible, 2);
    }
}
