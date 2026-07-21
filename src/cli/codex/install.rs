use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use toml_edit::{DocumentMut, Item};

use codex_mixin::CODEX_MIXIN_PROVIDER;
use codex_mixin::catalog::{
    codex_catalog_from_models_with_metadata,
    codex_oauth_proxy_catalog_from_models_with_metadata_for_provider,
};
use codex_mixin::config::GatewayConfig;
use codex_mixin::history::{
    migrate_history_from_mixin_provider, migrate_history_to_mixin_provider,
};
use codex_mixin::server::AppState;

use super::catalog::*;
use super::managed_config::*;
use crate::cli::atomic_file::write_atomic_if_changed;
use crate::cli::config_input::normalize_base_url;
use crate::cli::metadata::load_model_metadata_resolver;
use crate::cli::runtime::{load_runtime_metadata, pid_is_running};

pub(in crate::cli) struct InstallCodexOptions {
    pub(in crate::cli) requested_model: Option<String>,
    pub(in crate::cli) set_default: bool,
    pub(in crate::cli) codex_oauth_proxy: bool,
    pub(in crate::cli) config_path: Option<PathBuf>,
    pub(in crate::cli) catalog_path: Option<PathBuf>,
    pub(in crate::cli) base_url: Option<String>,
    pub(in crate::cli) web_search: String,
    pub(in crate::cli) env_key: Option<String>,
    pub(in crate::cli) no_env_key: bool,
}

pub(in crate::cli) async fn install_codex(options: InstallCodexOptions) -> anyhow::Result<()> {
    let InstallCodexOptions {
        requested_model,
        set_default,
        codex_oauth_proxy,
        config_path,
        catalog_path,
        base_url,
        web_search,
        env_key,
        no_env_key,
    } = options;
    let paths = resolve_codex_install_paths(config_path, catalog_path)?;
    let template = load_codex_install_template(&paths, codex_oauth_proxy)?;
    let gateway_config = GatewayConfig::from_env()?;
    let state = AppState::new(gateway_config.clone())?;
    let mut models = state.fetch_models().await?;
    if models.is_empty() {
        anyhow::bail!("upstream /v1/models returned no models");
    }
    let web_search_probe = state
        .probe_web_search_capabilities(&mut models, false)
        .await?;
    let metadata = load_model_metadata_resolver().await?;
    let catalog = if codex_oauth_proxy {
        codex_oauth_proxy_catalog_from_models_with_metadata_for_provider(
            &models,
            gateway_config.default_context_window,
            template.as_ref(),
            &metadata,
            gateway_config.provider_preset.as_str(),
        )
    } else {
        codex_catalog_from_models_with_metadata(
            &models,
            gateway_config.default_context_window,
            template.as_ref(),
            &metadata,
        )
    };
    let serialized_catalog = serde_json::to_vec_pretty(&catalog)?;
    let gateway_bind = match load_runtime_metadata()? {
        Some(runtime) if pid_is_running(runtime.pid)? => runtime.bind,
        _ => gateway_config.bind,
    };
    let gateway_base_url =
        normalize_base_url(base_url.unwrap_or_else(|| format!("http://{gateway_bind}/v1")))?;
    let env_key = if codex_oauth_proxy || no_env_key {
        None
    } else {
        env_key.or_else(|| {
            gateway_config
                .gateway_api_key
                .as_ref()
                .map(|_| "CODEX_GATEWAY_KEY".to_owned())
        })
    };

    let _config_lock = ManagedConfigLock::acquire(&paths.config)?;
    let raw_config = read_managed_config_for_install(&paths.config)?;
    let mut doc = if raw_config.trim().is_empty() {
        DocumentMut::new()
    } else {
        raw_config.parse::<DocumentMut>()?
    };
    let should_set_default = set_default || requested_model.is_some();
    let selected_model = if codex_oauth_proxy {
        if should_set_default {
            Some(select_codex_oauth_proxy_model(
                requested_model,
                &models,
                template.as_ref(),
                &doc,
                gateway_config.provider_preset.as_str(),
            )?)
        } else {
            None
        }
    } else if should_set_default {
        Some(select_codex_model(requested_model, &models, &doc)?)
    } else {
        None
    };
    upsert_codex_config(
        &mut doc,
        selected_model.as_deref(),
        &paths.catalog,
        &gateway_base_url,
        &web_search,
        env_key.as_deref(),
        codex_oauth_proxy,
    )?;
    let serialized_config = format!("{MANAGED_CONFIG_HEADER}\n{doc}");
    serialized_config.parse::<DocumentMut>()?;
    let expected_model_slugs = catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("generated Codex catalog has no models array"))?
        .iter()
        .map(|model| {
            model
                .get("slug")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("generated Codex model is missing slug"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let codex_home = paths
        .config
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?;
    write_managed_codex_files(
        &paths,
        &raw_config,
        &serialized_catalog,
        serialized_config.as_bytes(),
        || validate_codex_install(codex_home, &expected_model_slugs),
    )?;

    let history = migrate_history_to_mixin_provider(codex_home)?;
    println!("codex config updated: {}", paths.config.display());
    println!(
        "codex config backup: {}",
        managed_backup_path(&paths.config).display()
    );
    println!("model catalog written: {}", paths.catalog.display());
    println!("models installed: {}", models.len());
    println!("metadata entries loaded: {}", metadata.len());
    println!(
        "web search capabilities: {} supported, {} unsupported, {} failed",
        web_search_probe.supported, web_search_probe.unsupported, web_search_probe.failed
    );
    println!("provider: {CODEX_MIXIN_PROVIDER}");
    println!(
        "history migrated: {} JSONL files, {} SQLite rows",
        history.jsonl_files_changed, history.sqlite_rows_changed
    );
    if let Some(backup_root) = history.backup_root {
        println!("history backup: {}", backup_root.display());
    }
    if codex_oauth_proxy {
        println!("codex oauth proxy: enabled");
    }
    if let Some(selected_model) = selected_model {
        println!("default model: {selected_model}");
    } else {
        println!("default model: unchanged");
    }
    println!("base_url: {gateway_base_url}");
    if let Some(env_key) = env_key
        && !codex_oauth_proxy
    {
        println!("env_key: {env_key}");
    }
    println!("reload required: restart Codex app; for Codex CLI, start a new session");
    Ok(())
}

pub(in crate::cli) fn uninstall_codex(
    config_path: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let config_path = resolve_codex_config_path(config_path)?;
    let _config_lock = ManagedConfigLock::acquire(&config_path)?;
    let raw_config = if config_path.exists() {
        fs::read_to_string(&config_path)?
    } else {
        String::new()
    };
    if !is_managed_config(&raw_config) {
        anyhow::bail!(
            "Codex config is not managed by codex-mixin: {}",
            config_path.display()
        );
    }
    let managed_doc = raw_config.parse::<DocumentMut>()?;
    let managed_catalog_path = managed_catalog_path(&managed_doc, &config_path)?;
    if let Some(explicit_catalog_path) = catalog_path {
        let explicit_catalog_path = absolute_path(explicit_catalog_path)?;
        if explicit_catalog_path != managed_catalog_path {
            anyhow::bail!(
                "explicit catalog {} does not match managed config catalog {}",
                explicit_catalog_path.display(),
                managed_catalog_path.display()
            );
        }
    }
    let backup_path = managed_backup_path(&config_path);
    let absent_marker_path = managed_absent_marker_path(&config_path);
    let restored_provider = if backup_path.exists() {
        let backup = fs::read_to_string(&backup_path)?;
        let doc = backup.parse::<DocumentMut>()?;
        doc.get("model_provider")
            .and_then(Item::as_str)
            .unwrap_or("openai")
            .to_owned()
    } else if absent_marker_path.exists() {
        "openai".to_owned()
    } else {
        anyhow::bail!(
            "missing managed backup for {}; expected {}",
            config_path.display(),
            backup_path.display()
        );
    };
    if backup_path.exists() {
        fs::copy(&backup_path, &config_path)?;
        fs::remove_file(&backup_path)?;
        println!("codex config restored: {}", config_path.display());
    } else if absent_marker_path.exists() {
        if config_path.exists() {
            fs::remove_file(&config_path)?;
        }
        fs::remove_file(&absent_marker_path)?;
        println!("codex config removed; no previous config existed");
    }
    let codex_home = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?;
    let history = migrate_history_from_mixin_provider(codex_home, &restored_provider)?;
    println!("history provider restored: {restored_provider}");
    println!(
        "history restored: {} JSONL files, {} SQLite rows",
        history.jsonl_files_changed, history.sqlite_rows_changed
    );
    if let Some(backup_root) = history.backup_root {
        println!("history backup: {}", backup_root.display());
    }
    if managed_catalog_path.exists() {
        fs::remove_file(&managed_catalog_path)?;
        println!("model catalog removed: {}", managed_catalog_path.display());
    }
    println!("reload required: restart Codex app; for Codex CLI, start a new session");
    Ok(())
}

pub(in crate::cli) fn write_managed_codex_files(
    paths: &CodexInstallPaths,
    raw_config: &str,
    serialized_catalog: &[u8],
    serialized_config: &[u8],
    validate: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let config_existed = paths.config.exists();
    let previous_catalog = if paths.catalog.exists() {
        Some(fs::read(&paths.catalog)?)
    } else {
        None
    };
    let created_restore_point = !is_managed_config(raw_config);
    create_managed_config_restore_point(&paths.config, raw_config)?;
    let install_result = (|| -> anyhow::Result<()> {
        write_atomic_if_changed(&paths.catalog, serialized_catalog)?;
        write_atomic_if_changed(&paths.config, serialized_config)?;
        validate()
    })();
    let Err(install_error) = install_result else {
        return Ok(());
    };

    let mut rollback_errors = Vec::new();
    let config_rollback = if config_existed {
        write_atomic_if_changed(&paths.config, raw_config.as_bytes()).map(|_| ())
    } else if paths.config.exists() {
        fs::remove_file(&paths.config).map_err(Into::into)
    } else {
        Ok(())
    };
    if let Err(error) = config_rollback {
        rollback_errors.push(format!("restore config: {error}"));
    }
    let catalog_rollback = match previous_catalog {
        Some(previous_catalog) => {
            write_atomic_if_changed(&paths.catalog, &previous_catalog).map(|_| ())
        }
        None if paths.catalog.exists() => fs::remove_file(&paths.catalog).map_err(Into::into),
        None => Ok(()),
    };
    if let Err(error) = catalog_rollback {
        rollback_errors.push(format!("restore catalog: {error}"));
    }
    if created_restore_point {
        for restore_path in [
            managed_backup_path(&paths.config),
            managed_absent_marker_path(&paths.config),
        ] {
            if restore_path.exists()
                && let Err(error) = fs::remove_file(&restore_path)
            {
                rollback_errors.push(format!(
                    "remove restore point {}: {error}",
                    restore_path.display()
                ));
            }
        }
    }
    if rollback_errors.is_empty() {
        anyhow::bail!(
            "Codex rejected the managed configuration; installation rolled back: {install_error}"
        );
    }
    anyhow::bail!(
        "Codex rejected the managed configuration: {install_error}; rollback also failed: {}",
        rollback_errors.join("; ")
    )
}

pub(in crate::cli) fn validate_codex_install(
    codex_home: &Path,
    expected_model_slugs: &[String],
) -> anyhow::Result<()> {
    let codex_cli = resolve_codex_cli()?;
    let doctor = ProcessCommand::new(&codex_cli)
        .args(["doctor", "--json"])
        .env("CODEX_HOME", codex_home)
        .output()?;
    let doctor_report: serde_json::Value =
        serde_json::from_slice(&doctor.stdout).map_err(|error| {
            anyhow::anyhow!(
                "Codex doctor returned invalid JSON: {error}; stderr: {}",
                String::from_utf8_lossy(&doctor.stderr)
                    .chars()
                    .take(1000)
                    .collect::<String>()
            )
        })?;
    let config_check = doctor_report
        .pointer("/checks/config.load")
        .ok_or_else(|| anyhow::anyhow!("Codex doctor report has no config.load check"))?;
    if config_check
        .get("status")
        .and_then(serde_json::Value::as_str)
        != Some("ok")
    {
        anyhow::bail!("Codex config.load check failed: {config_check}");
    }
    let effective_provider = config_check
        .pointer("/details/model provider")
        .and_then(serde_json::Value::as_str);
    if effective_provider != Some(CODEX_MIXIN_PROVIDER) {
        anyhow::bail!(
            "Codex loaded model provider {:?}, expected {CODEX_MIXIN_PROVIDER}",
            effective_provider
        );
    }

    let models = ProcessCommand::new(&codex_cli)
        .args(["debug", "models"])
        .env("CODEX_HOME", codex_home)
        .output()?;
    if !models.status.success() {
        anyhow::bail!(
            "Codex failed to load the managed model catalog: {}",
            String::from_utf8_lossy(&models.stderr)
                .chars()
                .take(1000)
                .collect::<String>()
        );
    }
    let loaded_catalog: serde_json::Value = serde_json::from_slice(&models.stdout)
        .map_err(|error| anyhow::anyhow!("Codex model catalog output is invalid JSON: {error}"))?;
    let loaded_slugs = loaded_catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Codex model catalog output has no models array"))?
        .iter()
        .filter_map(|model| model.get("slug").and_then(serde_json::Value::as_str))
        .collect::<HashSet<_>>();
    let missing_slugs = expected_model_slugs
        .iter()
        .filter(|slug| !loaded_slugs.contains(slug.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_slugs.is_empty() {
        anyhow::bail!(
            "Codex did not load {} managed models: {}",
            missing_slugs.len(),
            missing_slugs.join(", ")
        );
    }
    Ok(())
}

pub(in crate::cli) fn resolve_codex_cli() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_CLI_PATH").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
        anyhow::bail!(
            "CODEX_CLI_PATH does not point to a file: {}",
            path.display()
        );
    }
    for path in [
        PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
        PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
    ] {
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join("codex");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    anyhow::bail!(
        "Codex CLI was not found; set CODEX_CLI_PATH or install Codex before installing Codex Mixin"
    )
}
