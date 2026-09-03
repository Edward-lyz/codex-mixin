use std::cell::Cell;
use std::collections::HashSet;
use std::io::{self, IsTerminal, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use codex_mixin::provider::catalog_model_slug;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use serde_json::Value;

mod actions;
mod events;
mod forms;
#[cfg(test)]
mod tests;
mod view;

use actions::*;
use events::*;
use view::*;
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);
const PROVIDER_ACTION_LABELS: [&str; 6] = [
    "a add",
    "u edit",
    "D delete",
    "e enable",
    "t test",
    "m discover",
];
const MODEL_ACTION_LABELS: [&str; 5] = ["[SAVE]", "[ALL]", "[NONE]", "[DISCOVER]", "[PROBE]"];
const USAGE_RANGE_LABELS: [&str; 4] = ["[1D]", "[7D]", "[30D]", "[ALL]"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Page {
    Dashboard,
    Setup,
    Providers,
    Models,
    Benchmark,
    Fusion,
    Integrations,
    System,
    Diagnostics,
}

impl Page {
    const ALL: [Self; 9] = [
        Self::Dashboard,
        Self::Setup,
        Self::Providers,
        Self::Models,
        Self::Benchmark,
        Self::Fusion,
        Self::Integrations,
        Self::System,
        Self::Diagnostics,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Home",
            Self::Setup => "Setup",
            Self::Providers => "Providers",
            Self::Models => "Models",
            Self::Benchmark => "Speed",
            Self::Fusion => "Fusion",
            Self::Integrations => "Apps",
            Self::System => "System",
            Self::Diagnostics => "Logs",
        }
    }

    fn tab_title(self, compact: bool) -> &'static str {
        if compact {
            return if self == Self::System {
                "Sys"
            } else {
                self.title()
            };
        }
        match self {
            Self::Dashboard => "\u{2302} Home",
            Self::Setup => "\u{2295} Setup",
            Self::Providers => "\u{25c6} Providers",
            Self::Models => "\u{2261} Models",
            Self::Benchmark => "\u{21af} Speed",
            Self::Fusion => "\u{2726} Fusion",
            Self::Integrations => "\u{25c7} Apps",
            Self::System => "\u{2699} System",
            Self::Diagnostics => "\u{2637} Logs",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StartPage {
    Dashboard,
    Setup,
}

#[derive(Debug)]
struct Snapshot {
    status: Value,
    providers: Vec<Value>,
    codex_install_mode: Option<String>,
    benchmark: Option<Value>,
    usage: Vec<Value>,
    models: Vec<Value>,
    fusion_profile: Option<Value>,
    refreshed_at: Instant,
}

impl Snapshot {
    async fn load(usage_range: usize) -> anyhow::Result<Self> {
        let status = run_json(&["info", "--json"]).await?;
        let configured = status
            .get("configured")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let provider_document = if configured {
            run_json(&["providers", "list", "--json"]).await?
        } else {
            Value::Null
        };
        let providers = provider_document
            .get("providers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let codex_install_mode = provider_document
            .get("codex_install_mode")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let gateway_running = status.get("gateway").and_then(Value::as_str) == Some("running");
        let benchmark = if gateway_running {
            run_json(&["benchmark", "status"])
                .await?
                .get("snapshot")
                .cloned()
                .filter(|snapshot| !snapshot.is_null())
        } else {
            None
        };
        let usage = if gateway_running {
            let usage = match usage_range {
                0 => run_json(&["usage", "--json", "--days", "1"]).await?,
                1 => run_json(&["usage", "--json", "--days", "7"]).await?,
                2 => run_json(&["usage", "--json", "--days", "30"]).await?,
                _ => run_json(&["usage", "--json"]).await?,
            };
            usage
                .as_array()
                .context("usage output is not an array")?
                .clone()
        } else {
            Vec::new()
        };
        let mut models = providers
            .iter()
            .filter(|provider| value_str(provider, "kind", "") == "configured")
            .filter(|provider| {
                provider
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .flat_map(|provider| {
                let provider_id = value_str(provider, "id", "");
                let cached = provider
                    .get("cached_models")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                provider
                    .get("selected_models")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Value::as_str)
                    .map(move |model_id| {
                        let display_name = cached
                            .iter()
                            .find(|model| value_str(model, "id", "") == model_id)
                            .map(|model| value_str(model, "display_name", model_id))
                            .unwrap_or(model_id);
                        serde_json::json!({
                            "id": catalog_model_slug(model_id, provider_id),
                            "display_name": display_name,
                        })
                    })
            })
            .collect::<Vec<_>>();
        if configured {
            models.extend(load_official_fusion_models().await?);
        }
        let fusion_profile = provider_document
            .get("fusion_profile")
            .cloned()
            .filter(|profile| !profile.is_null());
        Ok(Self {
            status,
            providers,
            codex_install_mode,
            benchmark,
            usage,
            models,
            fusion_profile,
            refreshed_at: Instant::now(),
        })
    }

    fn configured(&self) -> bool {
        self.status
            .get("configured")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    fn gateway_running(&self) -> bool {
        self.status.get("gateway").and_then(Value::as_str) == Some("running")
    }

    fn benchmark_running(&self) -> bool {
        self.benchmark
            .as_ref()
            .and_then(|snapshot| snapshot.get("status"))
            .and_then(Value::as_str)
            == Some("running")
    }
}

async fn load_official_fusion_models() -> anyhow::Result<Vec<Value>> {
    let home = std::env::var_os("HOME").context("HOME is required to load official models")?;
    let cache = PathBuf::from(home).join(".codex/models_cache.json");
    if !tokio::fs::try_exists(&cache).await? {
        return Ok(Vec::new());
    }
    let document: Value = serde_json::from_slice(&tokio::fs::read(&cache).await?)?;
    let models = document
        .get("models")
        .and_then(Value::as_array)
        .context("official models cache does not contain a models array")?;
    Ok(models
        .iter()
        .filter(|model| value_str(model, "visibility", "list") != "hide")
        .filter_map(|model| {
            let slug = model.get("slug").and_then(Value::as_str)?;
            Some(serde_json::json!({
                "id": format!("official:{slug}"),
                "display_name": value_str(model, "display_name", slug),
            }))
        })
        .collect())
}

struct App {
    page: Page,
    snapshot: Snapshot,
    provider_index: usize,
    model_index: usize,
    model_draft: HashSet<String>,
    usage_offset: usize,
    usage_range: usize,
    notice: String,
    notice_is_error: bool,
    diagnostics: String,
    diagnostics_scroll: u16,
    help_visible: bool,
    dialog: Option<Dialog>,
    busy: Option<&'static str>,
    benchmark_refreshed_at: Instant,
    setup: SetupForm,
    viewport: Cell<Rect>,
    integration_index: usize,
    system_index: usize,
    benchmark_timeout_seconds: u64,
    benchmark_output_tokens: u64,
    quota: Vec<Value>,
    fusion: FusionForm,
}

impl App {
    fn new(snapshot: Snapshot, start_page: StartPage) -> Self {
        let providers = snapshot.providers.clone();
        let configured = snapshot.configured();
        let fusion = FusionForm::new(&snapshot);
        let mut app = Self {
            page: match start_page {
                StartPage::Dashboard if configured => Page::Dashboard,
                StartPage::Dashboard => Page::Setup,
                StartPage::Setup => Page::Setup,
            },
            snapshot,
            provider_index: 0,
            model_index: 0,
            model_draft: HashSet::new(),
            usage_offset: 0,
            usage_range: 3,
            notice: "Ready".to_owned(),
            notice_is_error: false,
            diagnostics: "Press x to run a quick health check.".to_owned(),
            diagnostics_scroll: 0,
            help_visible: false,
            dialog: None,
            busy: None,
            benchmark_refreshed_at: Instant::now(),
            setup: SetupForm::new(&providers),
            viewport: Cell::new(Rect::default()),
            integration_index: 0,
            system_index: 0,
            benchmark_timeout_seconds: 120,
            benchmark_output_tokens: 100,
            quota: Vec::new(),
            fusion,
        };
        app.load_model_draft();
        app
    }

    fn selected_provider(&self) -> Option<&Value> {
        self.snapshot.providers.get(self.provider_index)
    }

    fn next_page(&mut self, offset: isize) {
        let current = Page::ALL
            .iter()
            .position(|page| *page == self.page)
            .unwrap_or(0);
        let count = Page::ALL.len() as isize;
        let next = (current as isize + offset).rem_euclid(count) as usize;
        self.page = Page::ALL[next];
    }

    fn clamp_provider_index(&mut self) {
        self.provider_index = self
            .provider_index
            .min(self.snapshot.providers.len().saturating_sub(1));
    }

    fn load_model_draft(&mut self) {
        self.model_index = 0;
        self.model_draft = self
            .selected_provider()
            .and_then(|provider| provider.get("selected_models"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
    }

    fn selected_models(&self) -> Vec<&Value> {
        self.selected_provider()
            .and_then(|provider| provider.get("cached_models"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .collect()
    }
}

#[derive(Debug)]
enum Dialog {
    AddProvider(AddProviderForm),
    EditProvider(EditProviderForm),
    ConfirmRemove(String),
    ConfirmOperation(ConfirmOperation),
    ConfirmDisableFusion(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfirmOperation {
    UninstallCodex,
    UninstallClaude,
    UninstallDsh,
    UninstallOpenCode,
    UninstallPi,
    Update,
    Repair,
}

impl ConfirmOperation {
    fn title(self) -> &'static str {
        match self {
            Self::UninstallCodex => "Restore Codex",
            Self::UninstallClaude => "Restore Claude Code",
            Self::UninstallDsh => "Remove DSH integration",
            Self::UninstallOpenCode => "Remove OpenCode integration",
            Self::UninstallPi => "Remove Pi integration",
            Self::Update => "Update Codex Mixin",
            Self::Repair => "Repair configuration",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::UninstallCodex => "Restore the Codex configuration saved before installation.",
            Self::UninstallClaude => "Remove managed Claude Code settings and restore the backup.",
            Self::UninstallDsh => "Remove codex-mixin from DSH settings and credentials.",
            Self::UninstallOpenCode => {
                "Remove the managed provider and gateway credential from OpenCode."
            }
            Self::UninstallPi => {
                "Remove the managed provider, gateway credential, and reporting hooks from Pi."
            }
            Self::Update => {
                "Replace this CLI with the latest GitHub release and restart the gateway."
            }
            Self::Repair => "Run doctor --fix --quick and apply safe repairs.",
        }
    }
}

const PROVIDER_PRESETS: [&str; 6] = [
    "baidu-oneapi",
    "openrouter",
    "deepseek",
    "opencode-go",
    "aws-bedrock",
    "custom",
];

fn is_aws_bedrock(preset: &str) -> bool {
    preset == "aws-bedrock"
}

#[derive(Debug, Default)]
struct AddProviderForm {
    preset_index: usize,
    focus: usize,
    id: String,
    display_name: String,
    base_url: String,
    website_url: String,
    api_key: String,
    aws_access_key_id: String,
    aws_secret_access_key: String,
    aws_session_token: String,
    aws_region: String,
    quota_username: String,
    quota_workspace_id: String,
    quota_auth_cookie: String,
    image_generation_path: String,
    baidu_auth_bridge: usize,
    baidu_code_report: bool,
    auxiliary_model_upstream: bool,
    existing_ids: HashSet<String>,
}

#[derive(Debug)]
struct SetupForm {
    provider: AddProviderForm,
    focus: usize,
    codex_mode: usize,
}

#[derive(Debug)]
struct EditProviderForm {
    focus: usize,
    id: String,
    preset: String,
    enabled: bool,
    display_name: String,
    base_url: String,
    website_url: String,
    website_url_configured: bool,
    image_generation_path: String,
    image_generation_configured: bool,
    api_key: String,
    api_key_configured: bool,
    clear_key: bool,
    aws_access_key_id: String,
    aws_secret_access_key: String,
    aws_session_token: String,
    aws_region: String,
    aws_sigv4_configured: bool,
    aws_session_token_configured: bool,
    clear_aws_session_token: bool,
    clear_aws_credentials: bool,
    quota_username: String,
    quota_workspace_id: String,
    original_quota_workspace_id: String,
    quota_auth_cookie: String,
    quota_auth_cookie_configured: bool,
    clear_quota: bool,
    baidu_auth_bridge: usize,
    baidu_code_report: bool,
    auxiliary_model_upstream: bool,
}

#[derive(Debug)]
struct FusionForm {
    profile_id: String,
    loaded_profile_id: Option<String>,
    panel_models: HashSet<String>,
    model_index: usize,
    judge_model: String,
    final_model: String,
    min_successful: usize,
    timeout_ms: u64,
    show_intermediate_results: bool,
    panel_tools_enabled: bool,
    editing_profile_id: bool,
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode().context("enable terminal raw mode")?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide) {
            let _ = disable_raw_mode();
            return Err(error).context("enter alternate screen");
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                let mut stdout = io::stdout();
                let _ = execute!(stdout, Show, DisableMouseCapture, LeaveAlternateScreen);
                return Err(error).context("create terminal");
            }
        };
        Ok(Self { terminal })
    }

    fn draw(&mut self, app: &App) -> anyhow::Result<()> {
        self.terminal
            .draw(|frame| render(frame, app))
            .context("draw terminal UI")?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            Show,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Action {
    None,
    Quit,
    Refresh,
    ToggleGateway,
    RestartGateway,
    ToggleProvider,
    TestProvider,
    DiscoverModels,
    ProbeModels,
    ApplyModels,
    AddProvider,
    EditProvider,
    RemoveProvider,
    MoveProviderUp,
    MoveProviderDown,
    ConfirmRemoveProvider,
    SubmitDialog,
    RunSetup,
    StartBenchmark,
    ConnectCodexOfficial,
    ConnectCodexCustom,
    ConnectClaude,
    ConnectDsh,
    ConnectOpenCode,
    ConnectPi,
    ConfirmUninstallCodex,
    ConfirmUninstallClaude,
    ConfirmUninstallDsh,
    ConfirmUninstallOpenCode,
    ConfirmUninstallPi,
    ConfirmUpdate,
    ConfirmRepair,
    RunConfirmedOperation,
    RefreshCatalog,
    ShowLogs,
    RefreshQuota,
    RunDoctor,
    SaveFusion,
    ConfirmDisableFusion,
    DisableFusion,
}

#[allow(clippy::cognitive_complexity)]
pub(super) async fn run(
    start_page: StartPage,
    installed_cli_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("the full-screen UI requires a terminal; use --no-tui for plain output")
    }

    let snapshot = Snapshot::load(3).await?;
    let mut app = App::new(snapshot, start_page);
    if let Some(path) = installed_cli_path {
        let bin = path
            .parent()
            .context("installed CLI target has no parent directory")?;
        let bin_on_path = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).any(|entry| entry == bin))
            .unwrap_or(false);
        let installation_notice = if bin_on_path {
            format!("Installed command at {}", path.display())
        } else {
            format!(
                "Installed at {}; add {} to PATH",
                path.display(),
                bin.display()
            )
        };
        set_notice(&mut app, !bin_on_path, &installation_notice);
    }
    let mut terminal = TerminalSession::enter()?;
    terminal.draw(&app)?;
    if app.snapshot.gateway_running() {
        refresh_quota(&mut terminal, &mut app).await;
        terminal.draw(&app)?;
    }

    loop {
        if app.page == Page::Benchmark
            && app.snapshot.benchmark_running()
            && app.benchmark_refreshed_at.elapsed() >= Duration::from_secs(1)
        {
            refresh_benchmark(&mut app).await;
            terminal.draw(&app)?;
            continue;
        }
        if app.snapshot.refreshed_at.elapsed() >= REFRESH_INTERVAL && app.busy.is_none() {
            refresh(&mut terminal, &mut app).await;
            terminal.draw(&app)?;
            continue;
        }
        let Some(event) = read_event().await? else {
            continue;
        };
        let action = handle_event(&mut app, event);
        match action {
            Action::None => {}
            Action::Quit => break,
            Action::Refresh => {
                refresh(&mut terminal, &mut app).await;
            }
            Action::ToggleGateway => {
                let args = if app.snapshot.gateway_running() {
                    vec!["service", "stop"]
                } else {
                    vec!["service", "start"]
                };
                run_action(&mut terminal, &mut app, "Updating gateway", &args, true).await;
            }
            Action::RestartGateway => {
                run_action(
                    &mut terminal,
                    &mut app,
                    "Restarting gateway",
                    &["service", "restart"],
                    true,
                )
                .await;
            }
            Action::ToggleProvider => {
                if let Some(provider) = app.selected_provider() {
                    let id = provider
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let kind = provider.get("kind").and_then(Value::as_str);
                    let enabled = provider
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if kind == Some("official") {
                        set_notice(&mut app, true, "The official provider is managed by Codex.");
                    } else if let Some(id) = id {
                        let operation = if enabled { "disable" } else { "enable" };
                        let changed = run_action(
                            &mut terminal,
                            &mut app,
                            "Updating provider",
                            &["providers", operation, &id],
                            true,
                        )
                        .await
                        .is_some();
                        if changed {
                            apply_provider_changes(&mut terminal, &mut app).await;
                        }
                    }
                }
            }
            Action::MoveProviderUp | Action::MoveProviderDown => {
                let offset = if action == Action::MoveProviderUp {
                    -1
                } else {
                    1
                };
                if let Some(id) = selected_configured_provider_id(&app) {
                    let configured_ids = app
                        .snapshot
                        .providers
                        .iter()
                        .filter(|provider| value_str(provider, "kind", "") == "configured")
                        .filter_map(|provider| provider.get("id").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    if let Some(current) = configured_ids.iter().position(|current| current == &id)
                    {
                        let target = (current as isize + offset)
                            .clamp(0, configured_ids.len().saturating_sub(1) as isize)
                            as usize;
                        if target != current {
                            let mut reordered = configured_ids;
                            reordered.swap(current, target);
                            let mut args = vec!["providers".to_owned(), "reorder".to_owned()];
                            args.extend(reordered);
                            if run_owned_action(
                                &mut terminal,
                                &mut app,
                                "Reordering providers",
                                args,
                                true,
                            )
                            .await
                            .is_some()
                            {
                                app.provider_index =
                                    (app.provider_index as isize + offset).max(0) as usize;
                                apply_provider_changes(&mut terminal, &mut app).await;
                            }
                        }
                    }
                }
            }
            Action::TestProvider => {
                if let Some(id) = selected_configured_provider_id(&app) {
                    run_action(
                        &mut terminal,
                        &mut app,
                        "Testing provider",
                        &["providers", "test", &id, "--json"],
                        false,
                    )
                    .await;
                }
            }
            Action::DiscoverModels => {
                if let Some(id) = selected_configured_provider_id(&app) {
                    run_action(
                        &mut terminal,
                        &mut app,
                        "Refreshing model catalog",
                        &["providers", "discover", &id],
                        true,
                    )
                    .await;
                    app.load_model_draft();
                }
            }
            Action::ProbeModels => {
                if let Some(id) = selected_configured_provider_id(&app) {
                    run_action(
                        &mut terminal,
                        &mut app,
                        "Probing model capabilities",
                        &["providers", "probe", &id],
                        true,
                    )
                    .await;
                }
            }
            Action::ApplyModels => {
                if let Some(id) = selected_configured_provider_id(&app) {
                    let mut models = app.model_draft.iter().cloned().collect::<Vec<_>>();
                    models.sort();
                    let mut args = vec!["providers".to_owned(), "select".to_owned(), id];
                    for model in models {
                        args.extend(["--model".to_owned(), model]);
                    }
                    let saved = run_owned_action(
                        &mut terminal,
                        &mut app,
                        "Saving model selection",
                        args,
                        true,
                    )
                    .await
                    .is_some();
                    if saved {
                        let restarted = run_action(
                            &mut terminal,
                            &mut app,
                            "Restarting gateway",
                            &["service", "restart"],
                            true,
                        )
                        .await
                        .is_some();
                        if restarted {
                            run_action(
                                &mut terminal,
                                &mut app,
                                "Refreshing Codex model catalog",
                                &["refresh-codex-catalog"],
                                true,
                            )
                            .await;
                        }
                    }
                    app.load_model_draft();
                }
            }
            Action::SaveFusion => match app.fusion.args(&app.snapshot.models) {
                Ok(args) => {
                    if run_owned_action(
                        &mut terminal,
                        &mut app,
                        "Saving Fusion profile",
                        args,
                        true,
                    )
                    .await
                    .is_some()
                    {
                        apply_provider_changes(&mut terminal, &mut app).await;
                        app.fusion = FusionForm::new(&app.snapshot);
                    }
                }
                Err(error) => set_notice(&mut app, true, &error.to_string()),
            },
            Action::ConfirmDisableFusion => {
                if let Some(id) = app.fusion.loaded_profile_id.clone() {
                    app.dialog = Some(Dialog::ConfirmDisableFusion(id));
                } else {
                    set_notice(&mut app, true, "No Fusion profile is configured.");
                }
            }
            Action::DisableFusion => {
                let id = match app.dialog.take() {
                    Some(Dialog::ConfirmDisableFusion(id)) => id,
                    _ => continue,
                };
                if run_owned_action(
                    &mut terminal,
                    &mut app,
                    "Disabling Fusion",
                    vec![
                        "fusion".to_owned(),
                        "delete".to_owned(),
                        "--id".to_owned(),
                        id,
                    ],
                    true,
                )
                .await
                .is_some()
                {
                    apply_provider_changes(&mut terminal, &mut app).await;
                    app.fusion = FusionForm::new(&app.snapshot);
                }
            }
            Action::AddProvider => {
                app.dialog = Some(Dialog::AddProvider(AddProviderForm::new(
                    &app.snapshot.providers,
                )));
            }
            Action::EditProvider => {
                if let Some(form) = app
                    .selected_provider()
                    .and_then(EditProviderForm::from_provider)
                {
                    app.dialog = Some(Dialog::EditProvider(form));
                } else {
                    set_notice(&mut app, true, "The official provider is read-only.");
                }
            }
            Action::RemoveProvider => {
                if let Some(id) = selected_configured_provider_id(&app) {
                    app.dialog = Some(Dialog::ConfirmRemove(id));
                }
            }
            Action::ConfirmRemoveProvider => {
                let id = match app.dialog.take() {
                    Some(Dialog::ConfirmRemove(id)) => id,
                    _ => continue,
                };
                let removed = run_owned_action(
                    &mut terminal,
                    &mut app,
                    "Removing provider",
                    vec!["providers".to_owned(), "remove".to_owned(), id],
                    true,
                )
                .await
                .is_some();
                if removed {
                    apply_provider_changes(&mut terminal, &mut app).await;
                }
                app.load_model_draft();
            }
            Action::SubmitDialog => {
                let submission = match app.dialog.as_ref() {
                    Some(Dialog::AddProvider(form)) => {
                        form.args().map(|args| ("Adding provider", args))
                    }
                    Some(Dialog::EditProvider(form)) => {
                        form.args().map(|args| ("Saving provider", args))
                    }
                    _ => continue,
                };
                match submission {
                    Ok((label, args)) => {
                        app.dialog = None;
                        let changed = run_owned_action(&mut terminal, &mut app, label, args, true)
                            .await
                            .is_some();
                        if changed {
                            apply_provider_changes(&mut terminal, &mut app).await;
                        }
                        app.load_model_draft();
                    }
                    Err(error) => set_notice(&mut app, true, &error.to_string()),
                }
            }
            Action::RunSetup => {
                let submission = app.setup.provider.args();
                match submission {
                    Ok(args) => {
                        let provider_id = app.setup.provider.provider_id();
                        let codex_mode = app.setup.codex_mode;
                        app.setup.provider.clear_secrets();
                        let added = run_owned_action(
                            &mut terminal,
                            &mut app,
                            "Adding provider",
                            args,
                            true,
                        )
                        .await
                        .is_some();
                        if added {
                            let discovered = run_action(
                                &mut terminal,
                                &mut app,
                                "Discovering provider models",
                                &["providers", "discover", &provider_id],
                                true,
                            )
                            .await
                            .is_some();
                            if discovered {
                                let started = run_action(
                                    &mut terminal,
                                    &mut app,
                                    "Starting local gateway",
                                    &["service", "restart"],
                                    true,
                                )
                                .await
                                .is_some();
                                if started {
                                    let codex_args = match codex_mode {
                                        0 => Some(&["connect", "codex", "--codex-oauth-proxy"][..]),
                                        1 => Some(&["connect", "codex", "--custom-only"][..]),
                                        _ => None,
                                    };
                                    let installed = if let Some(codex_args) = codex_args {
                                        run_action(
                                            &mut terminal,
                                            &mut app,
                                            "Installing Codex integration",
                                            codex_args,
                                            true,
                                        )
                                        .await
                                        .is_some()
                                    } else {
                                        true
                                    };
                                    if installed {
                                        let providers = app.snapshot.providers.clone();
                                        app.setup.reset(&providers);
                                        app.page = Page::Models;
                                        app.load_model_draft();
                                        set_notice(
                                            &mut app,
                                            false,
                                            "Setup complete. Review the selected models.",
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => set_notice(&mut app, true, &error.to_string()),
                }
            }
            Action::StartBenchmark => {
                let mut args = vec![
                    "benchmark".to_owned(),
                    "start".to_owned(),
                    "--timeout-seconds".to_owned(),
                    app.benchmark_timeout_seconds.to_string(),
                    "--target-output-tokens".to_owned(),
                    app.benchmark_output_tokens.to_string(),
                ];
                if let Some(id) = app
                    .selected_provider()
                    .and_then(|provider| provider.get("id"))
                    .and_then(Value::as_str)
                {
                    args.extend(["--provider".to_owned(), id.to_owned()]);
                }
                run_owned_action(&mut terminal, &mut app, "Starting benchmark", args, true).await;
            }
            Action::ConnectCodexOfficial => {
                run_action(
                    &mut terminal,
                    &mut app,
                    "Installing Codex official mode",
                    &["connect", "codex", "--codex-oauth-proxy"],
                    true,
                )
                .await;
            }
            Action::ConnectCodexCustom => {
                run_action(
                    &mut terminal,
                    &mut app,
                    "Installing Codex custom mode",
                    &["connect", "codex", "--custom-only"],
                    true,
                )
                .await;
            }
            Action::ConnectClaude => {
                run_action(
                    &mut terminal,
                    &mut app,
                    "Installing Claude Code connection",
                    &["connect", "claude"],
                    true,
                )
                .await;
            }
            Action::ConnectDsh => {
                run_action(
                    &mut terminal,
                    &mut app,
                    "Installing DSH connection",
                    &["connect", "dsh"],
                    true,
                )
                .await;
            }
            Action::ConnectOpenCode => {
                run_action(
                    &mut terminal,
                    &mut app,
                    "Installing OpenCode connection",
                    &["connect", "opencode"],
                    true,
                )
                .await;
            }
            Action::ConnectPi => {
                run_action(
                    &mut terminal,
                    &mut app,
                    "Installing Pi connection and reporting hooks",
                    &["connect", "pi"],
                    true,
                )
                .await;
            }
            Action::ConfirmUninstallCodex => {
                app.dialog = Some(Dialog::ConfirmOperation(ConfirmOperation::UninstallCodex));
            }
            Action::ConfirmUninstallClaude => {
                app.dialog = Some(Dialog::ConfirmOperation(ConfirmOperation::UninstallClaude));
            }
            Action::ConfirmUninstallDsh => {
                app.dialog = Some(Dialog::ConfirmOperation(ConfirmOperation::UninstallDsh));
            }
            Action::ConfirmUninstallOpenCode => {
                app.dialog = Some(Dialog::ConfirmOperation(
                    ConfirmOperation::UninstallOpenCode,
                ));
            }
            Action::ConfirmUninstallPi => {
                app.dialog = Some(Dialog::ConfirmOperation(ConfirmOperation::UninstallPi));
            }
            Action::ConfirmUpdate => {
                app.dialog = Some(Dialog::ConfirmOperation(ConfirmOperation::Update));
            }
            Action::ConfirmRepair => {
                app.dialog = Some(Dialog::ConfirmOperation(ConfirmOperation::Repair));
            }
            Action::RunConfirmedOperation => {
                let operation = match app.dialog.take() {
                    Some(Dialog::ConfirmOperation(operation)) => operation,
                    _ => continue,
                };
                let (label, args, refresh_after) = match operation {
                    ConfirmOperation::UninstallCodex => (
                        "Restoring Codex configuration",
                        &["connect", "remove", "codex"][..],
                        true,
                    ),
                    ConfirmOperation::UninstallClaude => (
                        "Restoring Claude Code configuration",
                        &["connect", "remove", "claude"][..],
                        true,
                    ),
                    ConfirmOperation::UninstallDsh => (
                        "Removing DSH integration",
                        &["connect", "remove", "dsh"][..],
                        true,
                    ),
                    ConfirmOperation::UninstallOpenCode => (
                        "Removing OpenCode integration",
                        &["connect", "remove", "opencode"][..],
                        true,
                    ),
                    ConfirmOperation::UninstallPi => (
                        "Removing Pi integration",
                        &["connect", "remove", "pi"][..],
                        true,
                    ),
                    ConfirmOperation::Update => ("Updating Codex Mixin", &["update"][..], false),
                    ConfirmOperation::Repair => (
                        "Repairing configuration",
                        &["doctor", "--fix", "--quick"][..],
                        true,
                    ),
                };
                let output = run_action(&mut terminal, &mut app, label, args, refresh_after).await;
                if operation == ConfirmOperation::Repair
                    && let Some(output) = output
                {
                    app.diagnostics = pretty_json_or_text(&output);
                    app.diagnostics_scroll = 0;
                    app.page = Page::Diagnostics;
                }
            }
            Action::RefreshCatalog => {
                run_action(
                    &mut terminal,
                    &mut app,
                    "Refreshing Codex model catalog",
                    &["refresh-codex-catalog"],
                    true,
                )
                .await;
            }
            Action::ShowLogs => {
                if let Some(output) = run_action(
                    &mut terminal,
                    &mut app,
                    "Loading gateway logs",
                    &["service", "logs", "-n", "200"],
                    false,
                )
                .await
                {
                    app.diagnostics = output;
                    app.diagnostics_scroll = 0;
                    app.page = Page::Diagnostics;
                }
            }
            Action::RefreshQuota => refresh_quota(&mut terminal, &mut app).await,
            Action::RunDoctor => {
                let output = run_action(
                    &mut terminal,
                    &mut app,
                    "Running health check",
                    &["doctor", "--quick", "--json"],
                    false,
                )
                .await;
                if let Some(output) = output {
                    app.diagnostics = pretty_json_or_text(&output);
                    app.diagnostics_scroll = 0;
                    app.page = Page::Diagnostics;
                }
            }
        }
        terminal.draw(&app)?;
    }
    Ok(())
}
