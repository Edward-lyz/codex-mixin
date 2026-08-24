use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use codex_mixin::CODEX_MIXIN_PROVIDER;
use serde_json::Value;
use toml_edit::{DocumentMut, Item};

use super::super::codex::{
    is_managed_catalog_model, is_managed_config, managed_catalog_path, managed_config_provider_id,
    request_app_server, resolve_codex_cli, resolve_codex_config_path,
};
use super::{DoctorCheck, DoctorFix, DoctorStatus, LiveGateway};

pub(super) struct ManagedIntegration {
    #[cfg(target_os = "macos")]
    pub(super) config_path: PathBuf,
    pub(super) codex_home: PathBuf,
    pub(super) managed_slugs: HashSet<String>,
}

pub(super) fn normalized_model_key(raw: &str) -> String {
    raw.strip_suffix("-custom").unwrap_or(raw).to_owned()
}

pub(super) fn check_codex_integration(
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
                    #[cfg(target_os = "macos")]
                    config_path: path,
                    codex_home,
                    managed_slugs: HashSet::new(),
                }),
            );
        }
    };
    let catalog_models = serde_json::from_slice::<Value>(&catalog_raw)
        .ok()
        .and_then(|catalog| catalog.get("models").and_then(Value::as_array).cloned());
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
                #[cfg(target_os = "macos")]
                config_path: path,
                codex_home,
                managed_slugs: HashSet::new(),
            }),
        );
    };

    let all_slugs: HashSet<String> = catalog_models
        .iter()
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .map(normalized_model_key)
        .collect();
    let mut managed_slugs: HashSet<String> = catalog_models
        .iter()
        .filter(|model| is_managed_catalog_model(model))
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
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
            #[cfg(target_os = "macos")]
            config_path: path,
            codex_home,
            managed_slugs,
        }),
    )
}

pub(super) fn preview_list(items: &[&String]) -> String {
    let mut preview: Vec<&str> = items.iter().take(5).map(|item| item.as_str()).collect();
    preview.sort_unstable();
    let mut text = preview.join(", ");
    if items.len() > 5 {
        text.push_str(", ...");
    }
    text
}

#[derive(Debug, Default)]
pub(super) struct AppServerProbe {
    pub(super) initialize_ok: bool,
    pub(super) user_agent: Option<String>,
    pub(super) reported_codex_home: Option<String>,
    pub(super) model_list_ok: bool,
    pub(super) model_ids: Vec<String>,
}

/// Open an `app-server` session against the Codex engine and call `model/list`
/// so doctor verifies what the Desktop picker actually sees, not only the catalog file.
pub(super) async fn check_codex_engine(
    managed: &ManagedIntegration,
    timeout: Duration,
) -> DoctorCheck {
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
    let reply = request_app_server(
        cli,
        codex_home,
        "model/list",
        Some(serde_json::json!({"limit": 1000, "includeHidden": false})),
        timeout,
        "codex-mixin-doctor",
    )?;
    let mut probe = AppServerProbe {
        initialize_ok: true,
        user_agent: reply.initialize.get("userAgent").map(|agent| match agent {
            Value::String(agent) => agent.clone(),
            other => other.to_string(),
        }),
        reported_codex_home: reply
            .initialize
            .get("codexHome")
            .and_then(Value::as_str)
            .map(str::to_owned),
        ..AppServerProbe::default()
    };
    if let Some(data) = reply.result.get("data").and_then(Value::as_array) {
        probe.model_list_ok = true;
        probe.model_ids = data
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();
    }
    Ok(probe)
}
