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
        let mut entry = json!({
            "name": claude_desktop::route_id(model),
            "labelOverride": format!("{} - {}", option["label"].as_str().unwrap_or(model), option["description"].as_str().unwrap_or("")),
            "supports1m": supports_1m,
            // Without prefer1m Desktop renders the model twice: once as-is and
            // once as an auto-generated "<route>[1m]" variant whose label is
            // identical, which reads as two duplicate models in the picker.
            "prefer1m": supports_1m,
        });
        // Claude Desktop only builds an effort selector for model IDs it knows.
        // Route IDs carry an opaque hex suffix, so without behavesAs no effort
        // menu is offered and requests never carry output_config.effort.
        if let Some(anchor) = effort_anchor(model) {
            entry["behavesAs"] = json!(anchor);
        }
        Ok(entry)
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

/// Maps a provider model slug to a Claude model ID whose client-side
/// capabilities Desktop already knows. Desktop derives the effort selector
/// (`thinking.effort_options`) from that anchor; the effort a user picks is
/// sent as `output_config.effort`, which the gateway forwards upstream as
/// `reasoning.effort`. Only Claude-family models are anchored: pretending an
/// unrelated model is a Claude one would also apply Claude's prompt profile.
fn effort_anchor(model: &str) -> Option<&'static str> {
    let model = model.to_ascii_lowercase();
    let family = if model.contains("opus") {
        "opus"
    } else if model.contains("sonnet") {
        "sonnet"
    } else if model.contains("haiku") {
        "haiku"
    } else {
        return None;
    };
    let (major, minor) = model_version(&model, family)?;
    // Anchor to the exact catalog entry so Desktop shows that model's own
    // recommended effort and thinking modes. Unknown versions stay unanchored:
    // a family-level fallback would advertise effort levels the upstream model
    // does not accept.
    Some(match (family, major, minor) {
        ("opus", 5, Some(5)) => "claude-opus-5-5",
        ("opus", 5, None) => "claude-opus-5",
        ("opus", 4, Some(8)) => "claude-opus-4-8",
        ("opus", 4, Some(7)) => "claude-opus-4-7",
        ("opus", 4, Some(6)) => "claude-opus-4-6",
        ("opus", 4, Some(5)) => "claude-opus-4-5",
        ("sonnet", 5, Some(5)) => "claude-sonnet-5-5",
        ("sonnet", 5, None) => "claude-sonnet-5",
        ("sonnet", 4, Some(6)) => "claude-sonnet-4-6",
        ("sonnet", 4, Some(5)) => "claude-sonnet-4-5",
        ("haiku", 5, Some(5)) => "claude-haiku-5-5",
        ("haiku", 4, Some(5)) => "claude-haiku-4-5",
        _ => return None,
    })
}

/// Extracts the first `major[.minor]` version following the family token.
/// Components are limited to two digits so a `-YYYYMMDD` date suffix is not
/// mistaken for a minor version.
fn model_version(model: &str, family: &str) -> Option<(u8, Option<u8>)> {
    let after_family = model.split_once(family)?.1.as_bytes();
    let start = after_family.iter().position(u8::is_ascii_digit)?;
    let (major, consumed) = version_component(&after_family[start..])?;
    let rest = &after_family[start + consumed..];
    let minor = rest
        .strip_prefix(b".")
        .or_else(|| rest.strip_prefix(b"-"))
        .and_then(version_component)
        .map(|(minor, _)| minor);
    Some((major, minor))
}

fn version_component(bytes: &[u8]) -> Option<(u8, usize)> {
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || digits > 2 {
        return None;
    }
    let component = std::str::from_utf8(&bytes[..digits]).ok()?.parse().ok()?;
    Some((component, digits))
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
