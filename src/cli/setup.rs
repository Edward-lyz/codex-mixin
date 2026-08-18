use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use codex_mixin::config::load_stored_config;
use codex_mixin::provider::ProviderPreset;
use console::style;

use super::codex::{InstallCodexOptions, install_codex, resolve_codex_install_paths};
use super::ducx_setup::ensure_managed_ducx;
use super::providers::{
    AddProviderOptions, UpdateProviderOptions, add_provider, discover_models_with_output,
    update_provider,
};
use super::service::restart;
use super::{CliProviderPreset, SetupCodexMode, next_step_line, progress_is_interactive, stage};

fn install_cli_command() -> anyhow::Result<()> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(());
    };
    let bin = PathBuf::from(home).join(".local/bin");
    std::fs::create_dir_all(&bin)?;
    let target = bin.join("codex-mixin");
    let source = std::env::current_exe()?;
    if source != target {
        std::fs::copy(source, &target)?;
    }
    println!("CLI command installed: {}", target.display());
    println!(
        "Add {} to PATH if `codex-mixin` is not found.",
        bin.display()
    );
    Ok(())
}

fn read_secret(prompt: &str) -> anyhow::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    #[cfg(unix)]
    if !ProcessCommand::new("stty").arg("-echo").status()?.success() {
        anyhow::bail!("failed to disable terminal echo for secret input")
    }
    let mut value = String::new();
    let read_result = io::stdin().read_line(&mut value);
    #[cfg(unix)]
    if !ProcessCommand::new("stty").arg("echo").status()?.success() {
        anyhow::bail!("failed to restore terminal echo after secret input")
    }
    println!();
    read_result?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        anyhow::bail!("API key cannot be empty")
    }
    Ok(value)
}

fn choose_setup_codex_mode(mode: Option<SetupCodexMode>) -> anyhow::Result<SetupCodexMode> {
    if let Some(mode) = mode {
        return Ok(mode);
    }
    if !io::stdin().is_terminal() {
        return Ok(SetupCodexMode::Skip);
    }
    println!("\nConnect Codex:");
    println!("  1. Official account mode - keep ChatGPT login, plugins, and cloud features");
    println!("  2. Custom models only - no official account required");
    println!("  3. Skip for now");
    print!("Choose [1-3]: ");
    io::stdout().flush()?;
    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    match choice.trim() {
        "1" => Ok(SetupCodexMode::Official),
        "2" => Ok(SetupCodexMode::Custom),
        "3" => Ok(SetupCodexMode::Skip),
        _ => anyhow::bail!("invalid Codex mode; choose 1, 2, or 3"),
    }
}

fn choose_setup_preset(preset: Option<CliProviderPreset>) -> anyhow::Result<CliProviderPreset> {
    if let Some(preset) = preset {
        return Ok(preset);
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "provider preset is required in non-interactive mode; pass --preset <preset>\navailable presets: {}\nexample: codex-mixin setup --preset baidu-oneapi",
            ProviderPreset::available_presets_csv()
        );
    }
    println!("Choose a provider preset:");
    for (index, preset) in ProviderPreset::ALL.iter().enumerate() {
        println!(
            "  {}. {} - {}",
            index + 1,
            preset.as_str(),
            preset.description()
        );
    }
    print!("Choose [1-{}]: ", ProviderPreset::ALL.len());
    io::stdout().flush()?;
    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let index = choice
        .trim()
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("invalid preset choice; enter a number"))?;
    ProviderPreset::ALL
        .get(index.saturating_sub(1))
        .copied()
        .filter(|_| index >= 1)
        .map(CliProviderPreset::from)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "invalid preset choice; choose a number between 1 and {}",
                ProviderPreset::ALL.len()
            )
        })
}

pub(super) async fn run(
    preset: Option<CliProviderPreset>,
    key: Option<String>,
    quota_username: Option<String>,
    codex_mode: Option<SetupCodexMode>,
    no_start: bool,
) -> anyhow::Result<()> {
    let preset = choose_setup_preset(preset)?.as_str();
    if no_start
        && matches!(
            codex_mode,
            Some(SetupCodexMode::Official | SetupCodexMode::Custom)
        )
    {
        anyhow::bail!("--no-start cannot be combined with Codex installation");
    }
    install_cli_command()?;
    let key = match key.or_else(|| std::env::var("CODEX_MIXIN_API_KEY").ok()) {
        Some(key) if !key.trim().is_empty() => key,
        _ if io::stdin().is_terminal() => read_secret(&format!("API key for {preset}: "))?,
        _ => anyhow::bail!(
            "API key is required; pass --key or set CODEX_MIXIN_API_KEY in non-interactive mode"
        ),
    };

    let quota_username = if preset == "baidu-oneapi" {
        match quota_username.or_else(|| std::env::var("CODEX_MIXIN_QUOTA_USERNAME").ok()) {
            Some(username) if !username.trim().is_empty() => Some(username),
            _ if io::stdin().is_terminal() => {
                print!("Baidu OneAPI quota username: ");
                io::stdout().flush()?;
                let mut username = String::new();
                io::stdin().read_line(&mut username)?;
                let username = username.trim().to_owned();
                if username.is_empty() {
                    anyhow::bail!("Baidu OneAPI quota username cannot be empty")
                }
                Some(username)
            }
            _ => anyhow::bail!(
                "Baidu OneAPI quota username is required; pass --quota-username or set CODEX_MIXIN_QUOTA_USERNAME in non-interactive mode"
            ),
        }
    } else {
        quota_username
    };

    let ducx_executable = if preset == "baidu-oneapi" {
        Some(
            stage(
                "Preparing managed DUCX authentication",
                ensure_managed_ducx(),
            )
            .await?,
        )
    } else {
        None
    };
    let existing_provider = load_stored_config()?.and_then(|config| {
        config
            .providers
            .into_iter()
            .find(|provider| provider.id == preset)
    });
    if let Some(existing_provider) = existing_provider {
        println!("Updating provider configuration: {preset}");
        if existing_provider.preset_id.as_deref() != Some(preset) {
            anyhow::bail!(
                "provider {preset} already exists with preset {}; choose another provider id",
                existing_provider.preset_id.as_deref().unwrap_or("custom")
            )
        }
        update_provider(UpdateProviderOptions {
            id: preset.to_owned(),
            key: Some(key),
            quota_username,
            baidu_auth_bridge: (preset == "baidu-oneapi").then(|| "ducx_loopback".to_owned()),
            ducx_executable,
            ..UpdateProviderOptions::default()
        })
        .await?;
    } else {
        println!("Adding provider configuration: {preset}");
        add_provider(AddProviderOptions {
            preset: preset.to_owned(),
            id: None,
            key,
            display_name: None,
            base_url: None,
            website_url: None,
            protocol: None,
            api_path: None,
            models_path: None,
            image_generation_path: None,
            quota_url: None,
            quota_username,
            quota_workspace_id: None,
            quota_auth_cookie: None,
            quota_currency: None,
            quota_parser: None,
            gateway_key: None,
            static_models: Vec::new(),
            header_env: Vec::new(),
            baidu_auth_bridge: (preset == "baidu-oneapi").then(|| "ducx_loopback".to_owned()),
            ducx_executable,
            baidu_code_report: None,
        })
        .await?;
    }
    stage(
        &format!("Refreshing provider models for {preset}"),
        discover_models_with_output(preset, true),
    )
    .await?;
    println!("Provider models refreshed.");

    let executable = std::env::current_exe()?.display().to_string();
    if no_start {
        println!("Provider configured. Next: {executable} service start");
        return Ok(());
    }

    stage(
        "Restarting gateway with the new provider configuration",
        restart(None, None, true),
    )
    .await?;
    println!("Gateway is ready.");
    let codex_mode = choose_setup_codex_mode(codex_mode)?;
    match codex_mode {
        SetupCodexMode::Official => {
            let paths = resolve_codex_install_paths(None, None)?;
            if !paths.models_cache.exists() {
                anyhow::bail!(
                    "official Codex catalog is missing at {}; sign in and open Codex once, then run `{executable} connect codex --codex-oauth-proxy`",
                    paths.models_cache.display()
                )
            }
            install_codex(InstallCodexOptions {
                requested_model: None,
                set_default: false,
                codex_oauth_proxy: true,
                custom_only: false,
                config_path: None,
                catalog_path: None,
                base_url: None,
                web_search: "live".to_owned(),
                env_key: None,
                no_env_key: false,
            })
            .await?;
        }
        SetupCodexMode::Custom => {
            install_codex(InstallCodexOptions {
                requested_model: None,
                set_default: true,
                codex_oauth_proxy: false,
                custom_only: true,
                config_path: None,
                catalog_path: None,
                base_url: None,
                web_search: "live".to_owned(),
                env_key: None,
                no_env_key: false,
            })
            .await?;
        }
        SetupCodexMode::Skip => {
            println!("Gateway started.");
            next_step_line(&format!(
                "Connect Codex later: {executable} connect codex --codex-oauth-proxy"
            ));
        }
    }
    println!();
    if progress_is_interactive() {
        println!("{}", style("Setup complete").green().bold());
    } else {
        println!("Setup complete.");
    }
    next_step_line("Restart the Codex app, or start a new Codex CLI session");
    next_step_line(&format!("Check status: {executable} info"));
    next_step_line(&format!("Diagnose issues: {executable} doctor"));
    Ok(())
}
