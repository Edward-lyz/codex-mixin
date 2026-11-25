//! Package and account operations for the Mixin-managed Pi profile.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::path::Path;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, anyhow, ensure};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::application::error::OperationError;
use crate::pi_plugins::{
    self, Bridge, NPM_REGISTRY, PI_BRIDGE_PLACEHOLDER_URL, PI_BRIDGE_PROVIDER_ID, PI_PACKAGE,
    PI_VERSION, is_managed_provider,
};
use crate::platform::{self, agent_cli};
use crate::provider::{
    ModelDiscoveryChanges, ProviderModel, ProviderModelSource, ProviderPreset, ProviderProtocol,
    apply_discovered_models,
};

mod market;
pub use market::{MarketPackage, search};

static SHARED_BRIDGE: LazyLock<Bridge> = LazyLock::new(Bridge::default);

#[derive(Serialize)]
pub struct Status {
    pub node_available: bool,
    pub node_version: Option<String>,
    pub runtime_initialized: bool,
    pub pi_version: Option<String>,
    pub installed_packages: Vec<String>,
    pub discovered_providers: Vec<String>,
    pub provider_details: Vec<PluginProvider>,
    pub discovered_models: Vec<String>,
    pub commands: Vec<PluginCommand>,
    pub bridge_error: Option<String>,
    pub extension_errors: usize,
    pub npm_registry: &'static str,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginProvider {
    pub id: String,
    #[serde(default)]
    pub mixin_provider_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub auth_status: String,
    #[serde(default)]
    pub using_oauth: bool,
    #[serde(default)]
    pub model_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginCommandArgument {
    pub value: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginCommand {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub arguments: Vec<PluginCommandArgument>,
}

#[derive(Deserialize)]
struct Settings {
    #[serde(default)]
    packages: Vec<PackageSource>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PackageSource {
    Direct(String),
    Filtered { source: String },
}

#[derive(Deserialize)]
struct BridgeCatalog {
    extension_errors: usize,
    data: Vec<PiModel>,
}

#[derive(Deserialize)]
struct BridgeHealth {
    providers: Vec<String>,
    #[serde(default)]
    provider_details: Vec<PluginProvider>,
    #[serde(default)]
    commands: Vec<PluginCommand>,
}

#[derive(Deserialize)]
struct PiModel {
    id: String,
    provider: String,
    #[serde(default)]
    provider_display_name: Option<String>,
    #[serde(default)]
    package: Option<String>,
    name: String,
    context_window: Option<u64>,
    max_tokens: Option<u64>,
    reasoning: Option<bool>,
    supports_image: Option<bool>,
    #[allow(dead_code)]
    auth_status: String,
}

struct DiscoveredPiProvider {
    pi_provider_id: String,
    mixin_provider_id: String,
    display_name: String,
    package: Option<String>,
    models: Vec<ProviderModel>,
}

pub fn mixin_provider_id_for(pi_provider_id: &str) -> String {
    let mut slug = String::with_capacity(pi_provider_id.len());
    let mut last_dash = false;
    for ch in pi_provider_id.chars() {
        let lowered = ch.to_ascii_lowercase();
        if lowered.is_ascii_alphanumeric() {
            slug.push(lowered);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        PI_BRIDGE_PROVIDER_ID.to_owned()
    } else {
        format!("pi-{slug}")
    }
}

pub async fn status() -> anyhow::Result<Status> {
    let root = pi_plugins::root()?;
    let node_version = match pi_plugins::node() {
        Ok(path) => {
            let mut command = Command::new(path);
            platform::prepare_background_tokio_command(&mut command);
            let output = tokio::time::timeout(
                Duration::from_secs(5),
                command.arg("--version").kill_on_drop(true).output(),
            )
            .await??;
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        }
        Err(_) => None,
    };
    let runtime_initialized = pi_plugins::pi_cli_path(&root).is_file();
    let installed_packages = installed_sources(&root).await?;
    let (
        discovered_providers,
        provider_details,
        discovered_models,
        commands,
        bridge_error,
        extension_errors,
    ) = if runtime_initialized && !installed_packages.is_empty() {
        match bridge_catalog().await {
            Ok((health, catalog)) => {
                let details = health
                    .provider_details
                    .into_iter()
                    .map(|mut detail| {
                        detail.mixin_provider_id = mixin_provider_id_for(&detail.id);
                        if detail.display_name.trim().is_empty() {
                            detail.display_name = detail.id.clone();
                        }
                        detail
                    })
                    .collect();
                (
                    health.providers,
                    details,
                    catalog.data.into_iter().map(|model| model.id).collect(),
                    health.commands,
                    None,
                    catalog.extension_errors,
                )
            }
            Err(error) => (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Some(error.to_string()),
                0,
            ),
        }
    } else {
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), None, 0)
    };
    Ok(Status {
        node_available: node_version.is_some(),
        node_version,
        runtime_initialized,
        pi_version: runtime_initialized.then(|| PI_VERSION.to_owned()),
        installed_packages,
        discovered_providers,
        provider_details,
        discovered_models,
        commands,
        bridge_error,
        extension_errors,
        npm_registry: NPM_REGISTRY,
    })
}

async fn bridge_catalog() -> anyhow::Result<(BridgeHealth, BridgeCatalog)> {
    let endpoint = SHARED_BRIDGE.endpoint().await?;
    let health = bridge_http()?
        .get(format!("{}/health", endpoint.base_url))
        .bearer_auth(&endpoint.token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok((health, catalog_at(&endpoint).await?))
}

pub async fn install(spec: &str) -> Result<usize, OperationError> {
    let spec = npm_source(spec).map_err(before_commit)?;
    let root = pi_plugins::root().map_err(before_commit)?;
    let _lock = operation_lock(&root).await.map_err(before_commit)?;
    bootstrap(&root).await.map_err(before_commit)?;
    quiet_pi(
        &root,
        &["install", &spec, "--no-approve"],
        "install Pi package",
    )
    .await
    .map_err(before_commit)?;
    after_change(&root).await
}

pub async fn remove(spec: &str) -> Result<usize, OperationError> {
    let spec = npm_source(spec).map_err(before_commit)?;
    let root = pi_plugins::root().map_err(before_commit)?;
    let _lock = operation_lock(&root).await.map_err(before_commit)?;
    quiet_pi(
        &root,
        &["remove", &spec, "--no-approve"],
        "remove Pi package",
    )
    .await
    .map_err(before_commit)?;
    // Drop the package's providers right away so a failing model sync cannot
    // leave them behind in the gateway configuration.
    if crate::config::stored_config_path().is_file() {
        let package = npm_name(&spec);
        crate::config::mutate_stored_config(|config| {
            // `spec` is always an npm source, so only npm sources can match.
            let owned_by_package = |provider: &crate::provider::ProviderDefinition| {
                is_managed_provider(provider)
                    && provider.pi_plugin_package.as_deref().is_some_and(|source| {
                        source.starts_with("npm:") && npm_name(source) == package
                    })
            };
            if config
                .providers
                .iter()
                .any(|provider| !owned_by_package(provider))
            {
                config
                    .providers
                    .retain(|provider| !owned_by_package(provider));
            } else {
                // The gateway needs at least one provider entry, so retire
                // the package's providers instead of deleting them.
                for provider in &mut config.providers {
                    provider.enabled = false;
                    provider.codex_thread_title_model = None;
                    provider.selected_models.clear();
                    let _ = apply_discovered_models(provider, Vec::new())?;
                }
            }
            super::routing::disable_unavailable_routes(config)?;
            Ok(())
        })
        .map_err(|source| OperationError::AfterCommit {
            stage: "Pi provider cleanup",
            source,
        })?;
    }
    after_change(&root).await
}

pub async fn update(spec: Option<&str>) -> Result<usize, OperationError> {
    let spec = spec.map(npm_source).transpose().map_err(before_commit)?;
    let root = pi_plugins::root().map_err(before_commit)?;
    let _lock = operation_lock(&root).await.map_err(before_commit)?;
    let args = match spec.as_deref() {
        Some(source) => vec!["update", "--extension", source, "--no-approve"],
        None => vec!["update", "--extensions", "--no-approve"],
    };
    quiet_pi(&root, &args, "update Pi packages")
        .await
        .map_err(before_commit)?;
    after_change(&root).await
}

pub async fn auth_login(provider: &str, no_browser: bool) -> Result<usize, OperationError> {
    let root = pi_plugins::root().map_err(before_commit)?;
    let _lock = operation_lock(&root).await.map_err(before_commit)?;
    require_registered_provider(provider)
        .await
        .map_err(before_commit)?;
    let mut command = pi_command(&root).map_err(before_commit)?;
    command.args(["auth", "login", provider]);
    if no_browser {
        command.arg("--no-browser");
    }
    platform::interactive_output_to_stderr(&mut command)
        .context("route isolated Pi sign-in prompts to stderr")
        .map_err(before_commit)?;
    command.kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(15 * 60), command.status())
        .await
        .context("isolated Pi sign-in timed out")
        .map_err(before_commit)?
        .context("run isolated Pi sign-in")
        .map_err(before_commit)?;
    if !status.success() {
        return Err(before_commit(anyhow!(
            "isolated Pi sign-in exited with {status}"
        )));
    }
    after_change(&root).await
}

pub async fn auth_logout(provider: &str) -> Result<usize, OperationError> {
    let root = pi_plugins::root().map_err(before_commit)?;
    let _lock = operation_lock(&root).await.map_err(before_commit)?;
    require_registered_provider(provider)
        .await
        .map_err(before_commit)?;
    quiet_pi(
        &root,
        &["auth", "logout", provider],
        "sign out of isolated Pi account",
    )
    .await
    .map_err(before_commit)?;
    after_change(&root).await
}

async fn require_registered_provider(provider: &str) -> anyhow::Result<()> {
    ensure!(
        !provider.is_empty()
            && !provider.starts_with('-')
            && provider.len() <= 256
            && provider
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)),
        "invalid Pi provider identifier"
    );
    let state = status().await?;
    ensure!(
        state.discovered_providers.iter().any(|id| id == provider),
        "Pi plugin provider {provider} is not loaded"
    );
    Ok(())
}

pub async fn list_commands() -> anyhow::Result<Vec<PluginCommand>> {
    if installed_sources(&pi_plugins::root()?).await?.is_empty() {
        return Ok(Vec::new());
    }
    let endpoint = SHARED_BRIDGE.endpoint().await?;
    Ok(bridge_http()?
        .get(format!("{}/admin/commands", endpoint.base_url))
        .bearer_auth(endpoint.token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

pub async fn run_command(name: &str, args: &str) -> anyhow::Result<serde_json::Value> {
    let root = pi_plugins::root()?;
    let _lock = operation_lock(&root).await?;
    let endpoint = SHARED_BRIDGE.endpoint().await?;
    let response = bridge_http()?
        .post(format!("{}/admin/commands/run", endpoint.base_url))
        .bearer_auth(endpoint.token)
        .json(&serde_json::json!({"name": name, "args": args}))
        .send()
        .await?;
    ensure!(
        response.status() != reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "Pi command requires interactive UI"
    );
    let result = response.error_for_status()?.json().await?;
    pi_plugins::invalidate(&root).await?;
    let _ = sync_models(&root).await;
    Ok(result)
}

pub async fn sync() -> anyhow::Result<usize> {
    Ok(sync_with_changes().await?.0)
}

pub(crate) async fn sync_with_changes() -> anyhow::Result<(usize, ModelDiscoveryChanges)> {
    let root = pi_plugins::root()?;
    let _lock = operation_lock(&root).await?;
    let result = sync_models(&root).await?;
    pi_plugins::invalidate(&root).await?;
    Ok(result)
}

async fn after_change(root: &Path) -> Result<usize, OperationError> {
    pi_plugins::invalidate(root)
        .await
        .map_err(|source| OperationError::AfterCommit {
            stage: "Pi runtime invalidation",
            source,
        })?;
    let count = sync_models(root)
        .await
        .map(|(count, _)| count)
        .map_err(|source| OperationError::AfterCommit {
            stage: "Pi model synchronization",
            source,
        })?;
    Ok(count)
}

fn before_commit(source: anyhow::Error) -> OperationError {
    OperationError::BeforeCommit { source }
}

async fn sync_models(root: &Path) -> anyhow::Result<(usize, ModelDiscoveryChanges)> {
    let providers = discover_provider_models(root).await?;
    let count = providers.iter().map(|provider| provider.models.len()).sum();
    let changes = tokio::task::spawn_blocking(move || save_provider_models(providers)).await??;
    Ok((count, changes))
}

pub(crate) async fn configured_models(
    pi_provider_id: Option<&str>,
) -> anyhow::Result<Vec<ProviderModel>> {
    let providers = discover_provider_models(&pi_plugins::root()?).await?;
    Ok(providers
        .into_iter()
        .filter(|provider| pi_provider_id.is_none_or(|id| provider.pi_provider_id == id))
        .flat_map(|provider| provider.models)
        .collect())
}

async fn discover_provider_models(root: &Path) -> anyhow::Result<Vec<DiscoveredPiProvider>> {
    if installed_sources(root).await?.is_empty() {
        return Ok(Vec::new());
    }
    let endpoint = SHARED_BRIDGE.endpoint().await?;
    bridge_http()?
        .post(format!("{}/admin/models/refresh", endpoint.base_url))
        .bearer_auth(&endpoint.token)
        .send()
        .await?
        .error_for_status()?;
    let catalog = catalog_at(&endpoint).await?;
    let mut grouped: BTreeMap<String, DiscoveredPiProvider> = BTreeMap::new();
    for model in catalog.data {
        let pi_provider_id = model.provider.clone();
        let display_name = model
            .provider_display_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| pi_provider_id.clone());
        let package = model.package.clone();
        let entry = grouped
            .entry(pi_provider_id.clone())
            .or_insert_with(|| DiscoveredPiProvider {
                mixin_provider_id: mixin_provider_id_for(&pi_provider_id),
                pi_provider_id,
                display_name,
                package,
                models: Vec::new(),
            });
        if entry.package.is_none() {
            entry.package = model.package.clone();
        }
        entry.models.push(ProviderModel::from(model));
    }
    Ok(grouped.into_values().collect())
}

impl From<PiModel> for ProviderModel {
    fn from(model: PiModel) -> Self {
        Self {
            id: model.id,
            display_name: Some(model.name),
            context_window: model.context_window,
            max_output_tokens: model.max_tokens,
            supports_thinking: model.reasoning,
            supports_image: model.supports_image,
            ..Self::default()
        }
    }
}

fn save_provider_models(
    providers: Vec<DiscoveredPiProvider>,
) -> anyhow::Result<ModelDiscoveryChanges> {
    if providers.is_empty() && !crate::config::stored_config_path().is_file() {
        return Ok(ModelDiscoveryChanges::default());
    }
    crate::config::mutate_stored_config(|config| {
        let mut total = ModelDiscoveryChanges::default();
        let active_ids: HashSet<String> = providers
            .iter()
            .map(|provider| provider.mixin_provider_id.clone())
            .collect();

        for discovered in providers {
            let existing = config.providers.iter().position(|provider| {
                provider.id == discovered.mixin_provider_id
                    || (is_managed_provider(provider)
                        && provider.pi_provider_id.as_deref()
                            == Some(discovered.pi_provider_id.as_str()))
            });
            if let Some(index) = existing {
                ensure!(
                    is_managed_provider(&config.providers[index]),
                    "provider ID {} is already in use",
                    discovered.mixin_provider_id
                );
                let provider = &mut config.providers[index];
                provider.display_name = discovered.display_name;
                provider.pi_provider_id = Some(discovered.pi_provider_id);
                provider.pi_plugin_package = discovered.package;
                if provider.cached_models.is_empty() {
                    provider.enabled = true;
                    provider.models_refreshed_at_ms = None;
                }
                if provider
                    .codex_thread_title_model
                    .as_ref()
                    .is_some_and(|id| {
                        !discovered.models.iter().any(|model| &model.id == id)
                            && !provider
                                .cached_models
                                .iter()
                                .any(|model| &model.id == id && model.manually_added)
                    })
                {
                    provider.codex_thread_title_model = None;
                }
                let changes = apply_discovered_models(provider, discovered.models)?;
                total.added.extend(changes.added);
                total.removed.extend(changes.removed);
                total.auto_selected.extend(changes.auto_selected);
            } else {
                let mut provider = ProviderPreset::Custom.create(&discovered.mixin_provider_id, "");
                provider.display_name = discovered.display_name;
                provider.base_url = PI_BRIDGE_PLACEHOLDER_URL.to_owned();
                provider.api_path = "/v1/chat/completions".to_owned();
                provider.protocol = ProviderProtocol::OpenAiChat;
                provider.model_source = ProviderModelSource::Static;
                provider.pi_provider_id = Some(discovered.pi_provider_id);
                provider.pi_plugin_package = discovered.package;
                let mut changes = apply_discovered_models(&mut provider, discovered.models)?;
                changes.added = provider
                    .cached_models
                    .iter()
                    .map(|model| model.id.clone())
                    .collect();
                changes.auto_selected = provider.selected_models.clone();
                total.added.extend(changes.added);
                total.removed.extend(changes.removed);
                total.auto_selected.extend(changes.auto_selected);
                config.providers.push(provider);
            }
        }

        let stale_indices: Vec<usize> = config
            .providers
            .iter()
            .enumerate()
            .filter(|(_, provider)| {
                is_managed_provider(provider) && !active_ids.contains(&provider.id)
            })
            .map(|(index, _)| index)
            .collect();
        for index in stale_indices.into_iter().rev() {
            if config.providers.len() > 1 {
                let removed_provider = config.providers.remove(index);
                total.removed.extend(
                    removed_provider
                        .cached_models
                        .into_iter()
                        .map(|model| model.id),
                );
            } else {
                let provider = &mut config.providers[index];
                total.removed.extend(
                    std::mem::take(&mut provider.cached_models)
                        .into_iter()
                        .map(|model| model.id),
                );
                provider.enabled = false;
                provider.codex_thread_title_model = None;
                let _ = apply_discovered_models(provider, Vec::new())?;
            }
        }

        super::routing::disable_unavailable_routes(config)?;
        Ok(total)
    })
}

async fn catalog_at(endpoint: &pi_plugins::BridgeEndpoint) -> anyhow::Result<BridgeCatalog> {
    Ok(bridge_http()?
        .get(format!("{}/v1/models", endpoint.base_url))
        .bearer_auth(&endpoint.token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

fn bridge_http() -> anyhow::Result<&'static reqwest::Client> {
    static HTTP: LazyLock<Result<reqwest::Client, reqwest::Error>> = LazyLock::new(|| {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(60))
            .build()
    });
    HTTP.as_ref()
        .map_err(|error| anyhow!("create Pi bridge HTTP client: {error}"))
}

async fn installed_sources(root: &Path) -> anyhow::Result<Vec<String>> {
    let settings = match tokio::fs::read(root.join("agent/settings.json")).await {
        Ok(bytes) => serde_json::from_slice::<Settings>(&bytes)
            .context("read isolated Pi package settings")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("read isolated Pi package settings"),
    };
    Ok(settings
        .packages
        .into_iter()
        .map(|package| match package {
            PackageSource::Direct(source) | PackageSource::Filtered { source } => source,
        })
        .collect())
}

fn npm_source(spec: &str) -> anyhow::Result<String> {
    let spec = spec.strip_prefix("npm:").unwrap_or(spec);
    let name = npm_name(spec);
    let version = spec
        .get(name.len()..)
        .and_then(|value| value.strip_prefix('@'));
    ensure!(
        valid_npm_name(name)
            && version.is_none_or(|value| {
                !value.is_empty()
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b".-_+^~".contains(&byte))
            })
            && spec.len() <= 256,
        "expected an npm package name, optionally followed by @version"
    );
    Ok(format!("npm:{spec}"))
}

fn npm_name(spec: &str) -> &str {
    let text = spec.strip_prefix("npm:").unwrap_or(spec);
    text.get(1..)
        .and_then(|suffix| suffix.find('@'))
        .map_or(text, |index| &text[..index + 1])
}

fn valid_npm_name(name: &str) -> bool {
    let valid_part = |part: &str| {
        part.as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
    };
    if let Some(scoped) = name.strip_prefix('@') {
        scoped
            .split_once('/')
            .is_some_and(|(scope, package)| valid_part(scope) && valid_part(package))
    } else {
        valid_part(name)
    }
}

async fn operation_lock(root: &Path) -> anyhow::Result<File> {
    let root = root.to_owned();
    tokio::task::spawn_blocking(move || {
        for path in [&root, &root.join("agent"), &root.join("workspace")] {
            platform::ensure_owner_only_dir(path)?;
        }
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("operation.lock"))?;
        platform::set_owner_only_mode_on(&file)?;
        platform::lock_exclusive(&file)?;
        Ok(file)
    })
    .await?
}

async fn bootstrap(root: &Path) -> anyhow::Result<()> {
    if pi_plugins::pi_cli_path(root).is_file() {
        return Ok(());
    }
    let npm = agent_cli::find_command("npm").context("Pi plugins require npm")?;
    let mut command = Command::new(npm);
    pi_plugins::configure_pi_command(&mut command, root);
    platform::prepare_background_tokio_command(&mut command);
    let spec = format!("{PI_PACKAGE}@{PI_VERSION}");
    command
        .args(["install", "--no-audit", "--no-fund", "--prefix"])
        .arg(root)
        .arg(spec);
    quiet_command(command, "install isolated Pi runtime").await?;
    ensure!(
        pi_plugins::pi_cli_path(root).is_file(),
        "npm did not install the Pi runtime CLI"
    );
    Ok(())
}

fn pi_command(root: &Path) -> anyhow::Result<Command> {
    let cli = pi_plugins::pi_cli_path(root);
    ensure!(cli.is_file(), "Pi plugin runtime is not initialized");
    let mut command = Command::new(pi_plugins::node()?);
    pi_plugins::configure_pi_command(&mut command, root);
    command.arg(cli);
    Ok(command)
}

async fn quiet_pi(root: &Path, args: &[&str], action: &str) -> anyhow::Result<()> {
    let mut command = pi_command(root)?;
    platform::prepare_background_tokio_command(&mut command);
    command.args(args);
    quiet_command(command, action).await
}

async fn quiet_command(mut command: Command, action: &str) -> anyhow::Result<()> {
    platform::isolate_tokio_process_group(&mut command);
    let mut child = command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| action.to_owned())?;
    let status = match tokio::time::timeout(Duration::from_secs(240), child.wait()).await {
        Ok(status) => status.with_context(|| action.to_owned())?,
        Err(_) => {
            platform::terminate_isolated_tokio_child(child.id(), &mut child).await;
            anyhow::bail!("{action} timed out");
        }
    };
    ensure!(status.success(), "{action} exited with {status}");
    Ok(())
}
