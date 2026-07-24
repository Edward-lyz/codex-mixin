use std::fs;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use codex_mixin::config::{GatewayConfig, load_stored_config, stored_config_path};
use codex_mixin::provider::{
    ProviderDefinition, ProviderModelSource, ProviderQuotaParser, ProviderReadinessStatus,
    ProviderRegistry, discover_provider_models, redact_provider_error,
};
use codex_mixin::server::AppState;
use futures_util::future::join_all;
use serde::Serialize;

use super::ConfigScope;
use super::codex::{is_managed_config, resolve_codex_config_path};
use super::runtime::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DoctorStatus {
    Ok,
    Warning,
    Error,
}

impl DoctorStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warning => "WARN",
            Self::Error => "ERROR",
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
struct DoctorReport {
    ok: bool,
    generated_at_ms: u64,
    config_path: String,
    checks: Vec<DoctorCheck>,
    providers: Vec<DoctorProviderCheck>,
    summary: DoctorSummary,
}

pub(super) async fn doctor(json_output: bool) -> anyhow::Result<()> {
    let path = stored_config_path();
    let mut checks = Vec::new();
    let stored = match load_stored_config() {
        Ok(Some(config)) => {
            checks.push(DoctorCheck {
                id: "stored_config".to_owned(),
                name: "Provider 配置文件".to_owned(),
                status: DoctorStatus::Ok,
                message: format!("已读取 {} 个 Provider", config.providers.len()),
                detail: Some(path.display().to_string()),
            });
            Some(config)
        }
        Ok(None) => {
            checks.push(DoctorCheck {
                id: "stored_config".to_owned(),
                name: "Provider 配置文件".to_owned(),
                status: DoctorStatus::Error,
                message: "配置文件不存在，请先新增 Provider".to_owned(),
                detail: Some(path.display().to_string()),
            });
            None
        }
        Err(error) => {
            checks.push(DoctorCheck {
                id: "stored_config".to_owned(),
                name: "Provider 配置文件".to_owned(),
                status: DoctorStatus::Error,
                message: "配置文件无法读取或解析".to_owned(),
                detail: Some(format!("{error:#}")),
            });
            None
        }
    };

    if path.exists() {
        checks.push(check_config_permissions(&path));
    }

    if stored.is_some() {
        match GatewayConfig::from_stored_config() {
            Ok(config) => checks.push(DoctorCheck {
                id: "runtime_config".to_owned(),
                name: "运行配置".to_owned(),
                status: DoctorStatus::Ok,
                message: format!(
                    "{} 个 Provider，监听地址 {}，环境变量不会覆盖运行配置",
                    config.providers.len(),
                    config.bind
                ),
                detail: None,
            }),
            Err(error) => checks.push(DoctorCheck {
                id: "runtime_config".to_owned(),
                name: "运行配置".to_owned(),
                status: DoctorStatus::Error,
                message: "配置结构、Provider 路由或 Fusion 引用校验失败".to_owned(),
                detail: Some(format!("{error:#}")),
            }),
        }
    }

    let providers = if let Some(config) = &stored {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()?;
        join_all(
            config
                .providers
                .iter()
                .cloned()
                .map(|provider| check_doctor_provider(client.clone(), provider)),
        )
        .await
    } else {
        Vec::new()
    };

    checks.push(check_gateway_runtime().await);
    checks.push(check_codex_integration());
    checks.push(check_gateway_log());

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
    let report = DoctorReport {
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
    };
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_doctor_report(&report);
    }
    Ok(())
}

fn check_config_permissions(path: &std::path::Path) -> DoctorCheck {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(path) {
            Ok(metadata) if metadata.permissions().mode() & 0o077 == 0 => DoctorCheck {
                id: "config_permissions".to_owned(),
                name: "配置文件权限".to_owned(),
                status: DoctorStatus::Ok,
                message: format!("{:o}", metadata.permissions().mode() & 0o777),
                detail: None,
            },
            Ok(metadata) => DoctorCheck {
                id: "config_permissions".to_owned(),
                name: "配置文件权限".to_owned(),
                status: DoctorStatus::Warning,
                message: "配置文件可被当前用户以外的账号读取".to_owned(),
                detail: Some(format!(
                    "当前权限 {:o}，建议 chmod 600 {}",
                    metadata.permissions().mode() & 0o777,
                    path.display()
                )),
            },
            Err(error) => DoctorCheck {
                id: "config_permissions".to_owned(),
                name: "配置文件权限".to_owned(),
                status: DoctorStatus::Error,
                message: "无法读取配置文件权限".to_owned(),
                detail: Some(error.to_string()),
            },
        }
    }
    #[cfg(not(unix))]
    {
        DoctorCheck {
            id: "config_permissions".to_owned(),
            name: "配置文件权限".to_owned(),
            status: DoctorStatus::Ok,
            message: "当前平台不使用 Unix 权限检查".to_owned(),
            detail: None,
        }
    }
}

async fn check_doctor_provider(
    client: reqwest::Client,
    provider: ProviderDefinition,
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
            message: "Provider 配置校验失败".to_owned(),
            detail: Some(format!("{error:#}")),
            ..base
        };
    }
    if !provider.enabled {
        return DoctorProviderCheck {
            status: DoctorStatus::Warning,
            message: "Provider 已停用，跳过网络检查".to_owned(),
            ..base
        };
    }
    if readiness.routable_model_count == 0 {
        return DoctorProviderCheck {
            status: DoctorStatus::Error,
            message: "没有已选择且当前可用的模型".to_owned(),
            detail: Some(readiness.issues.join(", ")),
            ..base
        };
    }
    if provider.model_source == ProviderModelSource::Static {
        return DoctorProviderCheck {
            message: format!(
                "静态模型配置正常，{} 个模型可路由；未发起付费推理",
                readiness.routable_model_count
            ),
            ..base
        };
    }
    let started = Instant::now();
    match discover_provider_models(&client, &provider).await {
        Ok(models) => DoctorProviderCheck {
            message: format!(
                "模型接口正常，返回 {} 个模型，耗时 {} ms；未发起付费推理",
                models.len(),
                started.elapsed().as_millis()
            ),
            detail: provider
                .models_refresh_error
                .as_ref()
                .map(|error| format!("历史刷新错误仍在缓存中，当前检查已恢复：{error}")),
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
                message: "模型接口连接或响应检查失败".to_owned(),
                detail: Some(error),
                ..base
            }
        }
    }
}

async fn check_gateway_runtime() -> DoctorCheck {
    let runtime = match load_runtime_metadata() {
        Ok(Some(runtime)) => runtime,
        Ok(None) => {
            return DoctorCheck {
                id: "gateway".to_owned(),
                name: "本地网关".to_owned(),
                status: DoctorStatus::Warning,
                message: "网关未启动".to_owned(),
                detail: None,
            };
        }
        Err(error) => {
            return DoctorCheck {
                id: "gateway".to_owned(),
                name: "本地网关".to_owned(),
                status: DoctorStatus::Error,
                message: "运行状态文件无法读取".to_owned(),
                detail: Some(format!("{error:#}")),
            };
        }
    };
    match pid_is_running(runtime.pid) {
        Ok(false) => DoctorCheck {
            id: "gateway".to_owned(),
            name: "本地网关".to_owned(),
            status: DoctorStatus::Warning,
            message: format!("发现失效的运行状态：PID {} 已不存在", runtime.pid),
            detail: Some(format!("绑定地址 {}", runtime.bind)),
        },
        Err(error) => DoctorCheck {
            id: "gateway".to_owned(),
            name: "本地网关".to_owned(),
            status: DoctorStatus::Error,
            message: format!("无法检查网关 PID {}", runtime.pid),
            detail: Some(format!("{error:#}")),
        },
        Ok(true) => {
            let url = format!("http://{}/healthz", runtime.bind);
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build();
            match client {
                Ok(client) => match client.get(&url).send().await {
                    Ok(response) if response.status().is_success() => DoctorCheck {
                        id: "gateway".to_owned(),
                        name: "本地网关".to_owned(),
                        status: DoctorStatus::Ok,
                        message: format!("运行中，PID {}，地址 {}", runtime.pid, runtime.bind),
                        detail: runtime.version.map(|version| format!("版本 {version}")),
                    },
                    Ok(response) => DoctorCheck {
                        id: "gateway".to_owned(),
                        name: "本地网关".to_owned(),
                        status: DoctorStatus::Error,
                        message: format!("healthz 返回 {}", response.status()),
                        detail: Some(url),
                    },
                    Err(error) => DoctorCheck {
                        id: "gateway".to_owned(),
                        name: "本地网关".to_owned(),
                        status: DoctorStatus::Error,
                        message: "进程存在但 healthz 无法访问".to_owned(),
                        detail: Some(format!("{error:#}")),
                    },
                },
                Err(error) => DoctorCheck {
                    id: "gateway".to_owned(),
                    name: "本地网关".to_owned(),
                    status: DoctorStatus::Error,
                    message: "无法创建本地网关检查请求".to_owned(),
                    detail: Some(format!("{error:#}")),
                },
            }
        }
    }
}

fn check_codex_integration() -> DoctorCheck {
    let path = match resolve_codex_config_path(None) {
        Ok(path) => path,
        Err(error) => {
            return DoctorCheck {
                id: "codex_config".to_owned(),
                name: "Codex 集成".to_owned(),
                status: DoctorStatus::Error,
                message: "无法定位 Codex 配置文件".to_owned(),
                detail: Some(format!("{error:#}")),
            };
        }
    };
    if !path.exists() {
        return DoctorCheck {
            id: "codex_config".to_owned(),
            name: "Codex 集成".to_owned(),
            status: DoctorStatus::Warning,
            message: "Codex 配置文件不存在，尚未安装到 Codex".to_owned(),
            detail: Some(path.display().to_string()),
        };
    }
    match fs::read_to_string(&path) {
        Ok(raw) if is_managed_config(&raw) => DoctorCheck {
            id: "codex_config".to_owned(),
            name: "Codex 集成".to_owned(),
            status: DoctorStatus::Ok,
            message: "Codex 当前由 codex-mixin 管理".to_owned(),
            detail: Some(path.display().to_string()),
        },
        Ok(_) => DoctorCheck {
            id: "codex_config".to_owned(),
            name: "Codex 集成".to_owned(),
            status: DoctorStatus::Warning,
            message: "Codex 尚未安装 codex-mixin 配置".to_owned(),
            detail: Some(path.display().to_string()),
        },
        Err(error) => DoctorCheck {
            id: "codex_config".to_owned(),
            name: "Codex 集成".to_owned(),
            status: DoctorStatus::Error,
            message: "Codex 配置文件无法读取".to_owned(),
            detail: Some(format!("{}: {error}", path.display())),
        },
    }
}

fn check_gateway_log() -> DoctorCheck {
    let path = default_log_file_path();
    match fs::metadata(&path) {
        Ok(metadata) if metadata.len() > 0 => DoctorCheck {
            id: "gateway_log".to_owned(),
            name: "运行日志".to_owned(),
            status: DoctorStatus::Ok,
            message: format!("日志可用，{} bytes", metadata.len()),
            detail: Some(path.display().to_string()),
        },
        Ok(_) => DoctorCheck {
            id: "gateway_log".to_owned(),
            name: "运行日志".to_owned(),
            status: DoctorStatus::Warning,
            message: "日志文件为空".to_owned(),
            detail: Some(path.display().to_string()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DoctorCheck {
            id: "gateway_log".to_owned(),
            name: "运行日志".to_owned(),
            status: DoctorStatus::Warning,
            message: "日志文件尚未创建".to_owned(),
            detail: Some(path.display().to_string()),
        },
        Err(error) => DoctorCheck {
            id: "gateway_log".to_owned(),
            name: "运行日志".to_owned(),
            status: DoctorStatus::Error,
            message: "日志文件无法读取".to_owned(),
            detail: Some(format!("{}: {error}", path.display())),
        },
    }
}

fn print_doctor_report(report: &DoctorReport) {
    println!("Codex Mixin 自动检测");
    println!("config: {}", report.config_path);
    for check in &report.checks {
        println!(
            "[{}] {}: {}",
            check.status.label(),
            check.name,
            check.message
        );
        if let Some(detail) = &check.detail {
            println!("  {detail}");
        }
    }
    for provider in &report.providers {
        println!(
            "[{}] Provider {}: {}",
            provider.status.label(),
            provider.provider_id,
            provider.message
        );
        if let Some(detail) = &provider.detail {
            println!("  {detail}");
        }
    }
    println!(
        "summary: {} ok, {} warnings, {} errors",
        report.summary.ok, report.summary.warnings, report.summary.errors
    );
    println!("doctor: {}", if report.ok { "ok" } else { "issues found" });
}

pub(super) async fn status(json_output: bool) -> anyhow::Result<()> {
    let config = GatewayConfig::from_stored_config()?;
    let metadata = load_daemon_metadata()?;
    let runtime = load_runtime_metadata()?;
    let daemon_status = match &metadata {
        Some(metadata) if pid_is_running(metadata.pid)? => "running",
        Some(_) => "stale",
        None => "not_started",
    };
    let runtime_running = runtime
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
    let bind = if runtime_running {
        runtime.as_ref().expect("live runtime metadata").bind
    } else {
        metadata
            .as_ref()
            .map_or(config.bind, |metadata| metadata.bind)
    };
    let url = format!("http://{bind}/healthz");
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?
        .get(&url)
        .send()
        .await;
    match response {
        Ok(response) if response.status().is_success() => {
            let endpoint = format!("http://{bind}/v1");
            let readiness = provider_readiness_summary(&config.providers);
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "daemon": daemon_status,
                        "pid": metadata.as_ref().map(|metadata| metadata.pid),
                        "log": metadata.as_ref().map(|metadata| metadata.log_file.clone()),
                        "gateway": "running",
                        "gateway_version": if runtime_running {
                            runtime
                                .as_ref()
                                .and_then(|metadata| metadata.version.as_deref())
                                .unwrap_or("unknown")
                        } else {
                            "unknown"
                        },
                        "bind": bind,
                        "healthz": url,
                        "endpoint": endpoint,
                        "provider_readiness": readiness.0,
                        "provider_counts": {
                            "total": config.providers.len(),
                            "healthy": readiness.1,
                            "degraded": readiness.2,
                            "disabled": readiness.3,
                        },
                        "providers": provider_readiness_values(&config.providers),
                    }))?
                );
            } else {
                println!("daemon: {}", daemon_status.replace('_', " "));
                if let Some(metadata) = &metadata {
                    println!("pid: {}", metadata.pid);
                    println!("log: {}", metadata.log_file.display());
                }
                println!(
                    "gateway-version: {}",
                    if runtime_running {
                        runtime
                            .as_ref()
                            .and_then(|metadata| metadata.version.as_deref())
                            .unwrap_or("unknown")
                    } else {
                        "unknown"
                    }
                );
                println!("gateway: running");
                println!("healthz: {url}");
                println!("endpoint: {endpoint}");
                println!("provider-readiness: {}", readiness.0);
                println!(
                    "providers: {} total, {} healthy, {} degraded, {} disabled",
                    config.providers.len(),
                    readiness.1,
                    readiness.2,
                    readiness.3,
                );
            }
            Ok(())
        }
        Ok(response) => anyhow::bail!("gateway unhealthy: {} returned {}", url, response.status()),
        Err(err) => anyhow::bail!("gateway not running at {url}: {err}"),
    }
}

fn provider_readiness_summary(
    providers: &[ProviderDefinition],
) -> (&'static str, usize, usize, usize) {
    let mut healthy = 0;
    let mut degraded = 0;
    let mut disabled = 0;
    for provider in providers {
        match provider.readiness().status {
            ProviderReadinessStatus::Healthy => healthy += 1,
            ProviderReadinessStatus::Degraded => degraded += 1,
            ProviderReadinessStatus::Disabled => disabled += 1,
        }
    }
    let status = if degraded > 0 {
        "degraded"
    } else if healthy > 0 {
        "healthy"
    } else {
        "disabled"
    };
    (status, healthy, degraded, disabled)
}

fn provider_readiness_values(providers: &[ProviderDefinition]) -> Vec<serde_json::Value> {
    providers
        .iter()
        .map(|provider| {
            serde_json::json!({
                "id": provider.id,
                "display_name": provider.display_name,
                "enabled": provider.enabled,
                "protocol": provider.protocol,
                "readiness": provider.readiness(),
            })
        })
        .collect()
}

pub(super) async fn models(json_output: bool) -> anyhow::Result<()> {
    let config = GatewayConfig::from_stored_config()?;
    let state = AppState::new(config)?;
    let models = state.fetch_models().await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&models)?);
    } else {
        for model in models {
            println!("{}", model.id);
        }
    }
    Ok(())
}

pub(super) async fn probe_web_search(force: bool, json_output: bool) -> anyhow::Result<()> {
    let config = GatewayConfig::from_stored_config()?;
    let state = AppState::new(config)?;
    let mut models = state.fetch_models().await?;
    let summary = state
        .probe_web_search_capabilities(&mut models, force)
        .await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("models attempted: {}", summary.attempted);
        println!("models cached: {}", summary.cached);
        println!("web search supported: {}", summary.supported);
        println!("web search unsupported: {}", summary.unsupported);
        println!("probes failed: {}", summary.failed);
        for capability in summary.results {
            let status = if capability.error.is_some() {
                "probe-failed"
            } else if capability.supported {
                "supported"
            } else {
                "unsupported"
            };
            println!("{}: {} ({})", capability.model, status, capability.evidence);
        }
    }
    Ok(())
}

pub(super) async fn quota(json_output: bool, provider_filter: Option<&str>) -> anyhow::Result<()> {
    let stored = load_stored_config()?
        .ok_or_else(|| anyhow::anyhow!("provider configuration is missing"))?;
    let registry = ProviderRegistry::new(stored.providers)?;
    if let Some(provider_id) = provider_filter
        && registry.provider(provider_id).is_none()
    {
        anyhow::bail!("unknown provider: {provider_id}");
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let mut results = Vec::new();
    for provider in registry.providers() {
        if provider_filter.is_some_and(|filter| filter != provider.id()) {
            continue;
        }
        let Some(url) = provider.quota_url() else {
            results.push(serde_json::json!({
                "provider_id": provider.id(),
                "currency": provider.quota_currency(),
                "value": null,
                "error": "quota endpoint is not configured",
                "stale_at": null,
            }));
            continue;
        };
        let result = async {
            let response = provider.apply_auth(client.get(url)).send().await?;
            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                anyhow::bail!("quota endpoint returned {status}: {body}");
            }
            let value: serde_json::Value = serde_json::from_str(&body)?;
            let used = quota_used_value(provider.quota_parser(), &value)?;
            Ok::<_, anyhow::Error>((used, value))
        }
        .await;
        match result {
            Ok((used, raw)) => results.push(serde_json::json!({
                "provider_id": provider.id(),
                "currency": provider.quota_currency(),
                "value": used,
                "error": null,
                "stale_at": null,
                "raw": raw,
            })),
            Err(error) => results.push(serde_json::json!({
                "provider_id": provider.id(),
                "currency": provider.quota_currency(),
                "value": null,
                "error": error.to_string(),
                "stale_at": null,
            })),
        }
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    for result in results {
        let provider_id = result["provider_id"].as_str().unwrap_or("<unknown>");
        if let Some(error) = result["error"].as_str() {
            println!("{provider_id}: error: {error}");
        } else {
            let value = &result["value"];
            let currency = result["currency"].as_str().unwrap_or("");
            println!("{provider_id}: {value} {currency}");
        }
    }
    Ok(())
}

fn quota_used_value(parser: ProviderQuotaParser, value: &serde_json::Value) -> anyhow::Result<f64> {
    let pointers: &[&str] = match parser {
        ProviderQuotaParser::BaiduOneApi => &["/data/used_quota"],
        ProviderQuotaParser::OpenRouter => &["/data/total_usage"],
        ProviderQuotaParser::Generic => &[
            "/data/used_quota",
            "/data/total_usage",
            "/data/used",
            "/data/spent",
            "/data/cost",
            "/used_quota",
            "/total_usage",
            "/used",
            "/spent",
            "/cost",
        ],
    };
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(json_f64))
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| anyhow::anyhow!("quota response does not contain a valid used amount"))
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

#[cfg(test)]
pub(super) fn summarize_quota_json(value: &serde_json::Value) -> String {
    for base in [
        "",
        "/data",
        "/quota",
        "/data/quota",
        "/usage",
        "/data/usage",
    ] {
        if let Some(used) = first_json_number(
            value,
            base,
            &[
                "used",
                "used_quota",
                "usage",
                "total_usage",
                "spent",
                "cost",
                "consumed",
            ],
        ) {
            let limit = first_json_number(
                value,
                base,
                &[
                    "limit",
                    "total",
                    "total_credits",
                    "quota",
                    "quota_limit",
                    "month_quota_limit",
                    "budget",
                ],
            );
            let remaining =
                first_json_number(value, base, &["remaining", "remaining_quota", "available"]);
            if let Some(limit) = limit {
                if let Some(remaining) = remaining {
                    return format!("quota used: {used} / {limit}, remaining: {remaining}");
                }
                return format!("quota used: {used} / {limit}");
            }
            return format!("quota used: {used}");
        }
    }
    for path in [
        "/remaining",
        "/quota/remaining",
        "/data/remaining",
        "/data/quota/remaining",
        "/data/available",
        "/data/used",
        "/data/total",
        "/data/ratio",
        "/balance",
        "/data/balance",
        "/data/quota",
        "/total_available",
    ] {
        if let Some(value) = value.pointer(path) {
            return format!("quota {path}: {value}");
        }
    }
    value.to_string()
}

#[cfg(test)]
pub(super) fn first_json_number(
    value: &serde_json::Value,
    base: &str,
    fields: &[&str],
) -> Option<serde_json::Number> {
    fields.iter().find_map(|field| {
        let pointer = if base.is_empty() {
            format!("/{field}")
        } else {
            format!("{base}/{field}")
        };
        value.pointer(&pointer).and_then(json_number)
    })
}

#[cfg(test)]
pub(super) fn json_number(value: &serde_json::Value) -> Option<serde_json::Number> {
    match value {
        serde_json::Value::Number(number) => Some(number.clone()),
        serde_json::Value::String(value) => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64),
        _ => None,
    }
}

pub(super) fn show_config(json_output: bool, scope: ConfigScope) -> anyhow::Result<()> {
    let path = stored_config_path();
    match scope {
        ConfigScope::Stored => {
            let stored = load_stored_config()?.unwrap_or_default();
            let providers = redacted_providers(&stored.providers);
            let value = serde_json::json!({
                "path": path,
                "config_version": stored.config_version,
                "gateway_bind": stored.gateway_bind,
                "gateway_api_key": stored.gateway_api_key.as_ref().map(|_| "<redacted>"),
                "providers": providers,
                "fusion_profiles": stored.fusion_profiles
            });
            print_config_value(json_output, &value)
        }
        ConfigScope::Effective => {
            let config = GatewayConfig::from_stored_config()?;
            let bind = match load_runtime_metadata()? {
                Some(runtime) if pid_is_running(runtime.pid)? => runtime.bind,
                _ => config.bind,
            };
            let providers = redacted_providers(&config.providers);
            let value = serde_json::json!({
                "path": path,
                "bind": bind.to_string(),
                "providers": providers,
                "official_image_generation_url": config.official_image_generation_url()?,
                "official_image_edit_url": config.official_image_edit_url()?,
                "official_responses_url": config.official_responses_url,
                "codex_auth_path": config.codex_auth_path,
                "gateway_api_key": config.gateway_api_key.as_ref().map(|_| "<redacted>"),
                "accept_codex_oauth": config.accept_codex_oauth,
                "thinking_mode": format!("{:?}", config.thinking_mode),
                "enable_web_search_tool": config.enable_web_search_tool,
                "web_search_tool_type": config.web_search_tool_type,
                "web_search_max_uses": config.web_search_max_uses
            });
            print_config_value(json_output, &value)
        }
    }
}

fn redacted_providers(
    providers: &[codex_mixin::provider::ProviderDefinition],
) -> Vec<serde_json::Value> {
    providers
        .iter()
        .map(|provider| {
            serde_json::json!({
                "id": provider.id,
                "display_name": provider.display_name,
                "enabled": provider.enabled,
                "preset_id": provider.preset_id,
                "protocol": provider.protocol,
                "base_url": provider.base_url,
                "api_path": provider.api_path,
                "model_source": provider.model_source,
                "api_key": if provider.auth.api_key.is_empty() { "<missing>" } else { "<redacted>" },
                "image_generation_path": provider.image_generation_path,
                "quota_url": provider.quota_url,
                "quota_username": provider.quota_username,
                "quota_currency": provider.quota_currency,
                "selected_models": provider.selected_models,
                "new_models": provider.new_models,
                "cached_models": provider.cached_models,
                "models_refreshed_at_ms": provider.models_refreshed_at_ms,
                "last_model_refresh_error": provider.models_refresh_error,
                "readiness": provider.readiness(),
            })
        })
        .collect()
}

pub(super) fn print_config_value(
    json_output: bool,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(value)?);
        return Ok(());
    }
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("config output must be an object"))?;
    for (key, value) in object {
        println!("{key}: {}", printable_json_value(value));
    }
    Ok(())
}

pub(super) fn printable_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => "<unset>".to_owned(),
        other => other.to_string(),
    }
}
