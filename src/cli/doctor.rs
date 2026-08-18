use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use codex_mixin::config::{GatewayConfig, load_stored_config, stored_config_path};
use codex_mixin::provider::{
    ProviderDefinition, ProviderModelSource, discover_provider_models, redact_provider_error,
};
use console::style;
use futures_util::future::join_all;
use serde::Serialize;

use super::runtime::*;

mod codex;
mod desktop;
mod log;
mod repair;

use codex::{check_codex_engine, check_codex_integration};
#[cfg(target_os = "macos")]
use desktop::check_desktop_apps;
use log::check_gateway_log;
use repair::apply_doctor_fixes;

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
    use std::collections::HashSet;

    use super::codex::{AppServerProbe, normalized_model_key};
    use super::desktop::parse_ps_etime;
    use super::log::{GATEWAY_START_MARKER, count_error_lines};

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
