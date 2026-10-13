use std::path::PathBuf;

use anyhow::Context;
use codex_mixin::clients::claude_desktop;
use codex_mixin::config::GatewayConfig;
use codex_mixin::gateway_access::GatewayClient;
use serde_json::{Value, json};

fn config_root(root: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    std::path::absolute(match root {
        Some(root) => root,
        None => codex_mixin::platform::claude_desktop_config_root()?,
    })
    .context("resolve Claude Desktop configuration root")
}

fn install_config(root: Option<PathBuf>, announce: bool) -> anyhow::Result<bool> {
    let root = config_root(root)?;
    let config = GatewayConfig::from_stored_config()?;
    let official = super::official_models::selected_official_models(&config)?;
    let (picker, _) = super::claude::claude_model_picker(&config, &official)?;
    let models = picker["options"].as_array().context("Claude model picker options must be an array")?.iter().map(|option| {
        let model = option["model"].as_str().context("Claude picker model is missing")?;
        let supports_1m = model.ends_with("[1m]");
        let model = model.strip_suffix("[1m]").unwrap_or(model);
        Ok(json!({
            "name": claude_desktop::route_id(model),
            "labelOverride": format!("{} - {}", option["label"].as_str().unwrap_or(model), option["description"].as_str().unwrap_or("")),
            "supports1m": supports_1m,
        }))
    }).collect::<anyhow::Result<Vec<Value>>>()?;
    let bind = super::runtime::effective_gateway_bind(&config)?;
    anyhow::ensure!(
        bind.ip().is_loopback(),
        "Claude Desktop routing requires a loopback gateway bind"
    );
    let url = format!("http://{bind}/claude-desktop");
    let key = config.require_client_key(GatewayClient::ClaudeDesktop)?;
    let changed = if announce {
        claude_desktop::install(&root, &url, &key, json!(models))?
    } else {
        claude_desktop::refresh(&root, &url, &key, json!(models))?
    };
    if announce {
        println!("Claude Desktop gateway: {url}");
        println!(
            "Claude Desktop profile: {}",
            root.join("Claude-3p/configLibrary").display()
        );
        println!("Keep the gateway running; fully quit and reopen Claude Desktop to apply");
    }
    Ok(changed)
}

pub(super) fn install(root: Option<PathBuf>) -> anyhow::Result<()> {
    codex_mixin::application::client::install_with_client_key(GatewayClient::ClaudeDesktop, || {
        install_config(root, true).map(|_| ())
    })
}

pub(super) fn is_installed() -> anyhow::Result<bool> {
    claude_desktop::is_managed(&config_root(None)?)
}

pub(super) fn sync_installed() -> anyhow::Result<bool> {
    if !is_installed()? {
        return Ok(false);
    }
    codex_mixin::application::client::install_with_client_key(GatewayClient::ClaudeDesktop, || {
        install_config(None, false)
    })
}

pub(super) fn uninstall(root: Option<PathBuf>) -> anyhow::Result<()> {
    claude_desktop::uninstall(&config_root(root)?)?;
    codex_mixin::config::revoke_gateway_client_key(GatewayClient::ClaudeDesktop)?;
    println!("Claude Desktop configuration restored; fully quit and reopen Claude Desktop");
    Ok(())
}

pub(super) fn status(root: Option<PathBuf>, json_output: bool) -> anyhow::Result<()> {
    let root = config_root(root)?;
    let managed = claude_desktop::is_managed(&root)?;
    if json_output {
        println!("{}", json!({"managed": managed, "config_root": root}));
    } else {
        println!(
            "Claude Desktop: {}",
            if managed { "managed" } else { "not managed" }
        );
    }
    Ok(())
}
