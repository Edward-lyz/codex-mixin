use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use codex_mixin::config::GatewayConfig;

pub(super) const GATEWAY_LOG_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// Log filter for the gateway process. `RUST_LOG` raises it, which is how the
/// per-request prompt-cache diagnostics (`prefix_state`, `reused_turns`) become
/// visible: `RUST_LOG=codex_mixin=debug`.
fn gateway_log_filter() -> tracing_subscriber::EnvFilter {
    env_filter_or_default("info")
}

/// Parent CLI commands keep stderr clean so spinners and stage lines stay readable.
/// Explicit `RUST_LOG` still wins for debugging.
fn parent_cli_log_filter() -> tracing_subscriber::EnvFilter {
    env_filter_or_default("warn")
}

fn env_filter_or_default(default: &str) -> tracing_subscriber::EnvFilter {
    match std::env::var("RUST_LOG") {
        Ok(value) if !value.trim().is_empty() => {
            let lower = value.to_ascii_lowercase();
            if default == "info"
                && !["info", "debug", "trace"]
                    .iter()
                    .any(|level| lower.contains(level))
            {
                tracing_subscriber::EnvFilter::new(default)
            } else {
                tracing_subscriber::EnvFilter::new(value)
            }
        }
        _ => tracing_subscriber::EnvFilter::new(default),
    }
}

pub(crate) fn init_tracing(log_file: Option<&Path>, quiet_parent_logs: bool) -> anyhow::Result<()> {
    if let Some(log_file) = log_file {
        rotate_gateway_log_if_needed(log_file, GATEWAY_LOG_MAX_BYTES)?;
        if let Some(parent) = log_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)?;
        #[cfg(unix)]
        fs::set_permissions(log_file, fs::Permissions::from_mode(0o600))?;
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_thread_ids(false)
            .with_thread_names(false)
            .with_file(true)
            .with_line_number(true)
            .with_env_filter(gateway_log_filter())
            .with_target(true)
            .with_writer(Mutex::new(file))
            .try_init()
            .map_err(|error| anyhow::anyhow!("failed to install tracing subscriber: {error}"))?;
    } else {
        let filter = if quiet_parent_logs {
            parent_cli_log_filter()
        } else {
            gateway_log_filter()
        };
        tracing_subscriber::fmt()
            .with_writer(io::stderr)
            // A redirected log is unreadable and unparseable with ANSI escapes
            // in it, so colour only an interactive terminal.
            .with_ansi(io::stderr().is_terminal())
            .with_env_filter(filter)
            .with_target(true)
            .with_file(true)
            .with_line_number(true)
            .with_thread_ids(true)
            .try_init()
            .map_err(|error| anyhow::anyhow!("failed to install tracing subscriber: {error}"))?;
    }
    Ok(())
}

pub(crate) fn rotate_gateway_log_if_needed(path: &Path, max_bytes: u64) -> anyhow::Result<()> {
    if !path.exists() || fs::metadata(path)?.len() < max_bytes {
        return Ok(());
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let mut backup_name = path.as_os_str().to_os_string();
    backup_name.push(".1");
    let backup = PathBuf::from(backup_name);
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    fs::rename(path, backup)?;
    Ok(())
}

pub(super) fn log_gateway_configuration(config: &GatewayConfig) {
    tracing::info!(
        config_path = %codex_mixin::config::stored_config_path().display(),
        bind = %config.bind,
        provider_count = config.providers.len(),
        gateway_auth = if config.gateway_api_key.is_some() {
            "configured"
        } else {
            "disabled"
        },
        "gateway configuration loaded from stored config; runtime environment overrides are disabled"
    );
    for provider in &config.providers {
        let readiness = provider.readiness();
        tracing::info!(
            provider_id = %provider.id,
            display_name = %provider.display_name,
            enabled = provider.enabled,
            protocol = ?provider.protocol,
            base_url = %sanitized_url(&provider.base_url),
            api_path = %sanitized_path(&provider.api_path),
            model_source = match &provider.model_source {
                codex_mixin::provider::ProviderModelSource::OpenAiCompatible { .. } => "open_ai_compatible",
                codex_mixin::provider::ProviderModelSource::BaiduOneApi => "baidu_oneapi",
                codex_mixin::provider::ProviderModelSource::AwsBedrock => "aws_bedrock",
                codex_mixin::provider::ProviderModelSource::Static => "static",
            },
            selected_models = provider.selected_models.len(),
            routable_models = readiness.routable_model_count,
            readiness = readiness.status.as_str(),
            "provider configuration loaded"
        );
    }
}

fn sanitized_path(raw: &str) -> &str {
    raw.split(['?', '#']).next().unwrap_or("<invalid-path>")
}

fn sanitized_url(raw: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(raw) else {
        return "<invalid-url>".to_owned();
    };
    url.set_query(None);
    url.set_fragment(None);
    if !url.username().is_empty() {
        let _ = url.set_username("<redacted>");
    }
    if url.password().is_some() {
        let _ = url.set_password(Some("<redacted>"));
    }
    url.to_string().trim_end_matches('/').to_owned()
}
