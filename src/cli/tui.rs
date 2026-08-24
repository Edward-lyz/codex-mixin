use std::cell::Cell;
use std::collections::HashSet;
use std::io::{self, IsTerminal, Stdout};
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Gauge, List, ListItem, Paragraph, Row, Table, TableState,
    Tabs, Wrap,
};
use serde_json::Value;
use tokio::process::Command;

const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Page {
    Dashboard,
    Setup,
    Providers,
    Models,
    Benchmark,
    Integrations,
    System,
    Diagnostics,
}

impl Page {
    const ALL: [Self; 8] = [
        Self::Dashboard,
        Self::Setup,
        Self::Providers,
        Self::Models,
        Self::Benchmark,
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
            Self::Integrations => "Apps",
            Self::System => "System",
            Self::Diagnostics => "Logs",
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
    refreshed_at: Instant,
}

impl Snapshot {
    async fn load() -> anyhow::Result<Self> {
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
            run_json(&["usage", "--json"])
                .await?
                .as_array()
                .context("usage output is not an array")?
                .clone()
        } else {
            Vec::new()
        };
        Ok(Self {
            status,
            providers,
            codex_install_mode,
            benchmark,
            usage,
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

struct App {
    page: Page,
    snapshot: Snapshot,
    provider_index: usize,
    model_index: usize,
    model_draft: HashSet<String>,
    usage_offset: usize,
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
}

impl App {
    fn new(snapshot: Snapshot, start_page: StartPage) -> Self {
        let providers = snapshot.providers.clone();
        let configured = snapshot.configured();
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfirmOperation {
    UninstallCodex,
    UninstallClaude,
    UninstallDsh,
    Update,
    Repair,
}

impl ConfirmOperation {
    fn title(self) -> &'static str {
        match self {
            Self::UninstallCodex => "Restore Codex",
            Self::UninstallClaude => "Restore Claude Code",
            Self::UninstallDsh => "Remove DSH integration",
            Self::Update => "Update Codex Mixin",
            Self::Repair => "Repair configuration",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::UninstallCodex => "Restore the Codex configuration saved before installation.",
            Self::UninstallClaude => "Remove managed Claude Code settings and restore the backup.",
            Self::UninstallDsh => "Remove codex-mixin from DSH settings and credentials.",
            Self::Update => {
                "Replace this CLI with the latest GitHub release and restart the gateway."
            }
            Self::Repair => "Run doctor --fix --quick and apply safe repairs.",
        }
    }
}

const PROVIDER_PRESETS: [&str; 5] = [
    "baidu-oneapi",
    "openrouter",
    "deepseek",
    "opencode-go",
    "custom",
];

#[derive(Debug, Default)]
struct AddProviderForm {
    preset_index: usize,
    focus: usize,
    id: String,
    display_name: String,
    base_url: String,
    website_url: String,
    api_key: String,
    quota_username: String,
    quota_workspace_id: String,
    quota_auth_cookie: String,
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
    display_name: String,
    base_url: String,
    website_url: String,
    api_key: String,
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
    ConfirmRemoveProvider,
    SubmitDialog,
    RunSetup,
    StartBenchmark,
    ConnectCodexOfficial,
    ConnectCodexCustom,
    ConnectClaude,
    ConnectDsh,
    ConfirmUninstallCodex,
    ConfirmUninstallClaude,
    ConfirmUninstallDsh,
    ConfirmUpdate,
    ConfirmRepair,
    RunConfirmedOperation,
    RefreshCatalog,
    ShowLogs,
    RefreshQuota,
    RunDoctor,
}

impl AddProviderForm {
    fn new(providers: &[Value]) -> Self {
        Self {
            existing_ids: providers
                .iter()
                .filter_map(|provider| provider.get("id"))
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            ..Self::default()
        }
    }

    fn preset(&self) -> &'static str {
        PROVIDER_PRESETS[self.preset_index]
    }

    fn active_fields(&self) -> &'static [usize] {
        match self.preset() {
            "custom" => &[0, 1, 2, 3, 4, 5],
            "baidu-oneapi" => &[0, 1, 5, 6],
            "opencode-go" => &[0, 1, 5, 7, 8],
            _ => &[0, 1, 5],
        }
    }

    fn provider_id(&self) -> String {
        if !self.id.trim().is_empty() {
            return self.id.trim().to_owned();
        }
        let base = self.preset();
        if !self.existing_ids.contains(base) {
            return base.to_owned();
        }
        let mut suffix = 2_u64;
        loop {
            let candidate = format!("{base}-{suffix}");
            if !self.existing_ids.contains(&candidate) {
                return candidate;
            }
            suffix = suffix.saturating_add(1);
        }
    }

    fn move_focus(&mut self, offset: isize) {
        let fields = self.active_fields();
        let position = fields
            .iter()
            .position(|field| *field == self.focus)
            .unwrap_or(0);
        self.focus =
            fields[(position as isize + offset).rem_euclid(fields.len() as isize) as usize];
    }

    fn focused_text(&mut self) -> Option<&mut String> {
        match self.focus {
            1 => Some(&mut self.id),
            2 => Some(&mut self.display_name),
            3 => Some(&mut self.base_url),
            4 => Some(&mut self.website_url),
            5 => Some(&mut self.api_key),
            6 => Some(&mut self.quota_username),
            7 => Some(&mut self.quota_workspace_id),
            8 => Some(&mut self.quota_auth_cookie),
            _ => None,
        }
    }

    fn args(&self) -> anyhow::Result<Vec<String>> {
        let preset = self.preset();
        anyhow::ensure!(!self.api_key.trim().is_empty(), "API key is required");
        if preset == "custom" {
            anyhow::ensure!(
                !self.display_name.trim().is_empty(),
                "display name is required"
            );
            anyhow::ensure!(!self.base_url.trim().is_empty(), "base URL is required");
        }
        if preset == "baidu-oneapi" {
            anyhow::ensure!(
                !self.quota_username.trim().is_empty(),
                "quota username is required"
            );
        }
        if preset == "opencode-go" {
            anyhow::ensure!(
                !self.quota_workspace_id.trim().is_empty()
                    && !self.quota_auth_cookie.trim().is_empty(),
                "workspace ID and auth cookie are required"
            );
        }
        let mut args = vec![
            "providers".to_owned(),
            "add".to_owned(),
            "--preset".to_owned(),
            preset.to_owned(),
            "--key".to_owned(),
            self.api_key.trim().to_owned(),
        ];
        let provider_id = self.provider_id();
        args.extend(["--id".to_owned(), provider_id]);
        let mut optional = Vec::new();
        if preset == "custom" {
            optional.extend([
                ("--display-name", self.display_name.as_str()),
                ("--base-url", self.base_url.as_str()),
                ("--website-url", self.website_url.as_str()),
            ]);
        } else if preset == "baidu-oneapi" {
            optional.push(("--quota-username", self.quota_username.as_str()));
        } else if preset == "opencode-go" {
            optional.extend([
                ("--quota-workspace-id", self.quota_workspace_id.as_str()),
                ("--quota-auth-cookie", self.quota_auth_cookie.as_str()),
            ]);
        }
        for (flag, value) in optional {
            if !value.trim().is_empty() {
                args.extend([flag.to_owned(), value.trim().to_owned()]);
            }
        }
        Ok(args)
    }

    fn clear_secrets(&mut self) {
        self.api_key.clear();
        self.quota_auth_cookie.clear();
    }
}

impl SetupForm {
    fn new(providers: &[Value]) -> Self {
        Self {
            provider: AddProviderForm::new(providers),
            focus: 0,
            codex_mode: 0,
        }
    }

    fn active_fields(&self) -> Vec<usize> {
        let mut fields = self.provider.active_fields().to_vec();
        fields.push(9);
        fields
    }

    fn move_focus(&mut self, offset: isize) {
        let fields = self.active_fields();
        let position = fields
            .iter()
            .position(|field| *field == self.focus)
            .unwrap_or(0);
        self.focus =
            fields[(position as isize + offset).rem_euclid(fields.len() as isize) as usize];
        if self.focus != 9 {
            self.provider.focus = self.focus;
        }
    }

    fn codex_mode_name(&self) -> &'static str {
        match self.codex_mode {
            0 => "Official account",
            1 => "Custom models only",
            _ => "Skip for now",
        }
    }

    fn reset(&mut self, providers: &[Value]) {
        *self = Self::new(providers);
    }
}

impl EditProviderForm {
    fn from_provider(provider: &Value) -> Option<Self> {
        (provider.get("kind").and_then(Value::as_str) == Some("configured")).then(|| Self {
            focus: 0,
            id: value_str(provider, "id", "-").to_owned(),
            display_name: value_str(provider, "display_name", "").to_owned(),
            base_url: value_str(provider, "base_url", "").to_owned(),
            website_url: value_str(provider, "website_url", "").to_owned(),
            api_key: String::new(),
        })
    }

    fn focused_text(&mut self) -> &mut String {
        match self.focus {
            0 => &mut self.display_name,
            1 => &mut self.base_url,
            2 => &mut self.website_url,
            _ => &mut self.api_key,
        }
    }

    fn args(&self) -> anyhow::Result<Vec<String>> {
        anyhow::ensure!(
            !self.display_name.trim().is_empty(),
            "display name is required"
        );
        anyhow::ensure!(!self.base_url.trim().is_empty(), "base URL is required");
        let mut args = vec![
            "providers".to_owned(),
            "update".to_owned(),
            self.id.clone(),
            "--display-name".to_owned(),
            self.display_name.trim().to_owned(),
            "--base-url".to_owned(),
            self.base_url.trim().to_owned(),
        ];
        if !self.website_url.trim().is_empty() {
            args.extend([
                "--website-url".to_owned(),
                self.website_url.trim().to_owned(),
            ]);
        }
        if !self.api_key.trim().is_empty() {
            args.extend(["--key".to_owned(), self.api_key.trim().to_owned()]);
        }
        Ok(args)
    }
}

#[allow(clippy::cognitive_complexity)]
pub(super) async fn run(start_page: StartPage) -> anyhow::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("the full-screen UI requires a terminal; use --no-tui for plain output")
    }

    let snapshot = Snapshot::load().await?;
    let mut app = App::new(snapshot, start_page);
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
            Action::Refresh => refresh(&mut terminal, &mut app).await,
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
            Action::ConfirmUninstallCodex => {
                app.dialog = Some(Dialog::ConfirmOperation(ConfirmOperation::UninstallCodex));
            }
            Action::ConfirmUninstallClaude => {
                app.dialog = Some(Dialog::ConfirmOperation(ConfirmOperation::UninstallClaude));
            }
            Action::ConfirmUninstallDsh => {
                app.dialog = Some(Dialog::ConfirmOperation(ConfirmOperation::UninstallDsh));
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

fn handle_event(app: &mut App, event: Event) -> Action {
    let key = match event {
        Event::Mouse(mouse) => return handle_mouse_event(app, mouse.kind, mouse.column, mouse.row),
        Event::Key(key) => key,
        _ => return Action::None,
    };
    if key.kind != KeyEventKind::Press {
        return Action::None;
    }
    if app.page == Page::Setup && app.dialog.is_none() {
        return handle_setup_event(app, key.code, key.modifiers);
    }
    if app.dialog.is_some() {
        return handle_dialog_event(app, key.code, key.modifiers);
    }
    if app.help_visible {
        app.help_visible = false;
        return Action::None;
    }
    match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('?') => {
            app.help_visible = true;
            Action::None
        }
        KeyCode::Tab => {
            app.next_page(1);
            Action::None
        }
        KeyCode::BackTab => {
            app.next_page(-1);
            Action::None
        }
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Char('c') => Action::RefreshQuota,
        KeyCode::Char('s') if app.page != Page::Models => Action::ToggleGateway,
        KeyCode::Char('R') => Action::RestartGateway,
        KeyCode::Char('x') => Action::RunDoctor,
        code => handle_page_event(app, code),
    }
}

#[allow(clippy::cognitive_complexity)]
fn handle_mouse_event(app: &mut App, kind: MouseEventKind, column: u16, row: u16) -> Action {
    if app.dialog.is_some() {
        return Action::None;
    }
    if matches!(kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
        let offset = if kind == MouseEventKind::ScrollUp {
            -1
        } else {
            1
        };
        return match app.page {
            Page::Dashboard => {
                app.usage_offset = (app.usage_offset as isize + offset)
                    .clamp(0, app.snapshot.usage.len().saturating_sub(1) as isize)
                    as usize;
                Action::None
            }
            Page::Providers | Page::Models | Page::Benchmark => {
                select_provider(app, offset);
                Action::None
            }
            Page::Diagnostics => {
                app.diagnostics_scroll =
                    (app.diagnostics_scroll as isize + offset * 3).max(0) as u16;
                Action::None
            }
            _ => Action::None,
        };
    }
    if kind != MouseEventKind::Down(MouseButton::Left) {
        return Action::None;
    }
    let area = app.viewport.get();
    let tabs_y = area.y + 4;
    if row >= tabs_y && row < tabs_y + 3 {
        let mut start = area.x;
        for page in Page::ALL {
            let end = start + page.title().len() as u16 + 2;
            if column >= start && column < end {
                app.page = page;
                return Action::None;
            }
            start = end + 1;
        }
    }
    let body = Rect::new(
        area.x,
        area.y + 7,
        area.width,
        area.height.saturating_sub(10),
    );
    if row < body.y || row >= body.y + body.height {
        return Action::None;
    }
    match app.page {
        Page::Setup => {
            let split = body.x + body.width.saturating_mul(38) / 100;
            if column <= split {
                return if row >= body.y + body.height.saturating_sub(4) {
                    Action::RunSetup
                } else {
                    Action::None
                };
            }
            if row <= body.y {
                return Action::None;
            }
            let line = row.saturating_sub(body.y + 1) as usize;
            let fields = app.setup.active_fields();
            if line < fields.len().saturating_sub(1) {
                app.setup.focus = fields[line];
                app.setup.provider.focus = app.setup.focus;
            } else if line == fields.len() {
                app.setup.focus = 9;
            }
            Action::None
        }
        Page::Providers => {
            if column < body.x + 30 && row > body.y {
                let index = row.saturating_sub(body.y + 1) as usize / 2;
                if index < app.snapshot.providers.len() {
                    app.provider_index = index;
                    app.load_model_draft();
                }
            } else if row >= body.y + 9 {
                let segment = usize::from(column.saturating_sub(body.x + 30)) * 6
                    / usize::from(body.width.saturating_sub(30).max(1));
                return match segment.min(5) {
                    0 => Action::AddProvider,
                    1 => Action::EditProvider,
                    2 => Action::RemoveProvider,
                    3 => Action::ToggleProvider,
                    4 => Action::TestProvider,
                    _ => Action::DiscoverModels,
                };
            }
            Action::None
        }
        Page::Models => {
            if row < body.y + 3 {
                let segment =
                    usize::from(column.saturating_sub(body.x)) * 5 / usize::from(body.width.max(1));
                return match segment.min(4) {
                    0 => Action::ApplyModels,
                    1 => {
                        app.model_draft = app
                            .selected_models()
                            .into_iter()
                            .map(|model| model_id(model).to_owned())
                            .collect();
                        Action::None
                    }
                    2 => {
                        app.model_draft.clear();
                        Action::None
                    }
                    3 => Action::DiscoverModels,
                    _ => Action::ProbeModels,
                };
            }
            if row > body.y + 4 {
                let index = row.saturating_sub(body.y + 5) as usize;
                if index < app.selected_models().len() {
                    app.model_index = index;
                    if column < body.x + 8 {
                        let model = model_id(app.selected_models()[index]).to_owned();
                        if !app.model_draft.remove(&model) {
                            app.model_draft.insert(model);
                        }
                    }
                }
            }
            Action::None
        }
        Page::Benchmark => {
            if row < body.y + 5 {
                Action::StartBenchmark
            } else {
                Action::None
            }
        }
        Page::Integrations => {
            let relative_row = row.saturating_sub(body.y);
            let card = (usize::from(relative_row) * 3 / usize::from(body.height.max(1))).min(2);
            let relative_column = column.saturating_sub(body.x);
            app.integration_index = match card {
                0 => (usize::from(relative_column) * 3 / usize::from(body.width.max(1))).min(2),
                1 => 3 + (usize::from(relative_column) * 2 / usize::from(body.width.max(1))).min(1),
                _ => 5 + (usize::from(relative_column) * 2 / usize::from(body.width.max(1))).min(1),
            };
            integration_action(app.integration_index)
        }
        Page::System => {
            let right = column >= body.x + body.width / 2;
            let bottom = row >= body.y + body.height / 2;
            app.system_index = usize::from(bottom) * 2 + usize::from(right);
            if bottom && !right {
                let within_card = column.saturating_sub(body.x);
                if within_card < body.width / 4 {
                    Action::RunDoctor
                } else {
                    Action::ConfirmRepair
                }
            } else if bottom {
                let within_card = column.saturating_sub(body.x + body.width / 2);
                if within_card < body.width / 4 {
                    Action::RefreshCatalog
                } else {
                    Action::ShowLogs
                }
            } else {
                system_action(app.system_index)
            }
        }
        Page::Dashboard | Page::Diagnostics => Action::None,
    }
}

fn integration_action(index: usize) -> Action {
    match index {
        0 => Action::ConnectCodexOfficial,
        1 => Action::ConnectCodexCustom,
        2 => Action::ConfirmUninstallCodex,
        3 => Action::ConnectClaude,
        4 => Action::ConfirmUninstallClaude,
        5 => Action::ConnectDsh,
        _ => Action::ConfirmUninstallDsh,
    }
}

fn system_action(index: usize) -> Action {
    match index {
        0 => Action::ToggleGateway,
        1 => Action::ConfirmUpdate,
        2 => Action::ConfirmRepair,
        _ => Action::RefreshCatalog,
    }
}

fn handle_page_event(app: &mut App, code: KeyCode) -> Action {
    match app.page {
        Page::Setup => Action::None,
        Page::Providers => handle_provider_event(app, code),
        Page::Models => handle_model_event(app, code),
        Page::Benchmark => handle_benchmark_event(app, code),
        Page::Integrations => match code {
            KeyCode::Left | KeyCode::Up => {
                app.integration_index = app.integration_index.saturating_sub(1);
                Action::None
            }
            KeyCode::Right | KeyCode::Down => {
                app.integration_index = (app.integration_index + 1).min(6);
                Action::None
            }
            KeyCode::Enter => integration_action(app.integration_index),
            KeyCode::Char('1') => Action::ConnectCodexOfficial,
            KeyCode::Char('2') => Action::ConnectCodexCustom,
            KeyCode::Char('3') => Action::ConfirmUninstallCodex,
            KeyCode::Char('4') => Action::ConnectClaude,
            KeyCode::Char('5') => Action::ConfirmUninstallClaude,
            KeyCode::Char('6') => Action::ConnectDsh,
            KeyCode::Char('7') => Action::ConfirmUninstallDsh,
            _ => Action::None,
        },
        Page::System => match code {
            KeyCode::Left if app.system_index % 2 == 1 => {
                app.system_index -= 1;
                Action::None
            }
            KeyCode::Right if app.system_index.is_multiple_of(2) => {
                app.system_index += 1;
                Action::None
            }
            KeyCode::Up if app.system_index >= 2 => {
                app.system_index -= 2;
                Action::None
            }
            KeyCode::Down if app.system_index < 2 => {
                app.system_index += 2;
                Action::None
            }
            KeyCode::Enter => system_action(app.system_index),
            KeyCode::Char('u') => Action::ConfirmUpdate,
            KeyCode::Char('f') => Action::RefreshCatalog,
            KeyCode::Char('d') => Action::RunDoctor,
            KeyCode::Char('F') => Action::ConfirmRepair,
            KeyCode::Char('l') => Action::ShowLogs,
            _ => Action::None,
        },
        Page::Diagnostics => match code {
            KeyCode::PageUp => {
                app.diagnostics_scroll = app.diagnostics_scroll.saturating_sub(10);
                Action::None
            }
            KeyCode::PageDown => {
                app.diagnostics_scroll = app.diagnostics_scroll.saturating_add(10);
                Action::None
            }
            _ => Action::None,
        },
        Page::Dashboard => match code {
            KeyCode::Up => {
                app.usage_offset = app.usage_offset.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                app.usage_offset =
                    (app.usage_offset + 1).min(app.snapshot.usage.len().saturating_sub(1));
                Action::None
            }
            _ => Action::None,
        },
    }
}

fn handle_setup_event(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Action {
    if code == KeyCode::Esc {
        app.page = Page::Dashboard;
        return Action::None;
    }
    if code == KeyCode::Enter
        || (code == KeyCode::Char('s') && modifiers.contains(KeyModifiers::CONTROL))
    {
        return Action::RunSetup;
    }
    match code {
        KeyCode::Tab => app.setup.move_focus(1),
        KeyCode::BackTab => app.setup.move_focus(-1),
        KeyCode::Left if app.setup.focus == 0 => {
            app.setup.provider.preset_index = app.setup.provider.preset_index.saturating_sub(1);
        }
        KeyCode::Right if app.setup.focus == 0 => {
            app.setup.provider.preset_index =
                (app.setup.provider.preset_index + 1).min(PROVIDER_PRESETS.len() - 1);
        }
        KeyCode::Left if app.setup.focus == 9 => {
            app.setup.codex_mode = app.setup.codex_mode.saturating_sub(1);
        }
        KeyCode::Right if app.setup.focus == 9 => {
            app.setup.codex_mode = (app.setup.codex_mode + 1).min(2);
        }
        KeyCode::Backspace if app.setup.focus != 9 => {
            if let Some(value) = app.setup.provider.focused_text() {
                value.pop();
            }
        }
        KeyCode::Char(character) if app.setup.focus != 9 => {
            if let Some(value) = app.setup.provider.focused_text() {
                value.push(character);
            }
        }
        _ => {}
    }
    Action::None
}

fn handle_provider_event(app: &mut App, code: KeyCode) -> Action {
    match code {
        KeyCode::Up => {
            select_provider(app, -1);
            Action::None
        }
        KeyCode::Down => {
            select_provider(app, 1);
            Action::None
        }
        KeyCode::Char('e') => Action::ToggleProvider,
        KeyCode::Char('a') => Action::AddProvider,
        KeyCode::Char('u') => Action::EditProvider,
        KeyCode::Char('D') => Action::RemoveProvider,
        KeyCode::Char('t') => Action::TestProvider,
        KeyCode::Char('m') => Action::DiscoverModels,
        _ => Action::None,
    }
}

fn handle_model_event(app: &mut App, code: KeyCode) -> Action {
    match code {
        KeyCode::Up => app.model_index = app.model_index.saturating_sub(1),
        KeyCode::Down => {
            app.model_index =
                (app.model_index + 1).min(app.selected_models().len().saturating_sub(1));
        }
        KeyCode::Char('[') => select_provider(app, -1),
        KeyCode::Char(']') => select_provider(app, 1),
        KeyCode::Char(' ') => {
            if let Some(model) = app
                .selected_models()
                .get(app.model_index)
                .map(|model| model_id(model).to_owned())
                && !app.model_draft.remove(&model)
            {
                app.model_draft.insert(model);
            }
        }
        KeyCode::Char('a') => {
            app.model_draft = app
                .selected_models()
                .into_iter()
                .map(|model| model_id(model).to_owned())
                .collect();
        }
        KeyCode::Char('n') => app.model_draft.clear(),
        KeyCode::Char('d') => return Action::DiscoverModels,
        KeyCode::Char('p') => return Action::ProbeModels,
        KeyCode::Char('s') => return Action::ApplyModels,
        _ => {}
    }
    Action::None
}

fn handle_benchmark_event(app: &mut App, code: KeyCode) -> Action {
    match code {
        KeyCode::Char('[') => select_provider(app, -1),
        KeyCode::Char(']') => select_provider(app, 1),
        KeyCode::Char('-') => {
            app.benchmark_timeout_seconds =
                app.benchmark_timeout_seconds.saturating_sub(30).max(30);
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.benchmark_timeout_seconds = (app.benchmark_timeout_seconds + 30).min(600);
        }
        KeyCode::Char(',') => {
            app.benchmark_output_tokens = app.benchmark_output_tokens.saturating_sub(25).max(25);
        }
        KeyCode::Char('.') => {
            app.benchmark_output_tokens = (app.benchmark_output_tokens + 25).min(1_000);
        }
        KeyCode::Char('b') => return Action::StartBenchmark,
        _ => {}
    }
    Action::None
}

fn select_provider(app: &mut App, offset: isize) {
    let last = app.snapshot.providers.len().saturating_sub(1) as isize;
    app.provider_index = (app.provider_index as isize + offset).clamp(0, last) as usize;
    app.load_model_draft();
}

fn handle_dialog_event(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Action {
    if code == KeyCode::Esc {
        app.dialog = None;
        return Action::None;
    }
    let submit = code == KeyCode::Enter
        || (code == KeyCode::Char('s') && modifiers.contains(KeyModifiers::CONTROL));
    let Some(dialog) = app.dialog.as_mut() else {
        return Action::None;
    };
    match dialog {
        Dialog::ConfirmRemove(_) => match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Action::ConfirmRemoveProvider,
            KeyCode::Char('n') | KeyCode::Char('N') => {
                app.dialog = None;
                Action::None
            }
            _ => Action::None,
        },
        Dialog::ConfirmOperation(_) => match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Action::RunConfirmedOperation,
            KeyCode::Char('n') | KeyCode::Char('N') => {
                app.dialog = None;
                Action::None
            }
            _ => Action::None,
        },
        Dialog::AddProvider(form) => {
            if submit {
                return Action::SubmitDialog;
            }
            match code {
                KeyCode::Tab => form.move_focus(1),
                KeyCode::BackTab => form.move_focus(-1),
                KeyCode::Left if form.focus == 0 => {
                    form.preset_index = form.preset_index.saturating_sub(1);
                }
                KeyCode::Right if form.focus == 0 => {
                    form.preset_index = (form.preset_index + 1).min(PROVIDER_PRESETS.len() - 1);
                }
                KeyCode::Backspace => {
                    if let Some(value) = form.focused_text() {
                        value.pop();
                    }
                }
                KeyCode::Char(character) => {
                    if let Some(value) = form.focused_text() {
                        value.push(character);
                    }
                }
                _ => {}
            }
            Action::None
        }
        Dialog::EditProvider(form) => {
            if submit {
                return Action::SubmitDialog;
            }
            match code {
                KeyCode::Tab => form.focus = (form.focus + 1) % 4,
                KeyCode::BackTab => form.focus = (form.focus + 3) % 4,
                KeyCode::Backspace => {
                    form.focused_text().pop();
                }
                KeyCode::Char(character) => form.focused_text().push(character),
                _ => {}
            }
            Action::None
        }
    }
}

async fn refresh(terminal: &mut TerminalSession, app: &mut App) {
    app.busy = Some("Refreshing status");
    let _ = terminal.draw(app);
    match Snapshot::load().await {
        Ok(snapshot) => {
            app.snapshot = snapshot;
            app.clamp_provider_index();
            app.usage_offset = app
                .usage_offset
                .min(app.snapshot.usage.len().saturating_sub(1));
            set_notice(app, false, "Status refreshed.");
        }
        Err(error) => set_notice(app, true, &format!("Refresh failed: {error:#}")),
    }
    app.busy = None;
}

async fn refresh_benchmark(app: &mut App) {
    match run_json(&["benchmark", "status"]).await {
        Ok(document) => {
            app.snapshot.benchmark = document
                .get("snapshot")
                .cloned()
                .filter(|snapshot| !snapshot.is_null());
        }
        Err(error) => set_notice(app, true, &format!("Benchmark refresh failed: {error:#}")),
    }
    app.benchmark_refreshed_at = Instant::now();
}

async fn refresh_quota(terminal: &mut TerminalSession, app: &mut App) {
    app.busy = Some("Refreshing provider quota");
    let _ = terminal.draw(app);
    match run_json(&["quota", "--json"]).await {
        Ok(document) => match document.as_array() {
            Some(rows) => {
                app.quota.clone_from(rows);
                set_notice(app, false, "Provider quota refreshed.");
            }
            None => set_notice(app, true, "Quota refresh returned a non-array response."),
        },
        Err(error) => set_notice(app, true, &format!("Quota refresh failed: {error:#}")),
    }
    app.busy = None;
}

async fn run_action(
    terminal: &mut TerminalSession,
    app: &mut App,
    label: &'static str,
    args: &[&str],
    refresh_after: bool,
) -> Option<String> {
    app.busy = Some(label);
    let _ = terminal.draw(app);
    let output = match run_cli(args).await {
        Ok(output) => {
            set_notice(app, false, &format!("{label} completed."));
            app.diagnostics = pretty_json_or_text(&output);
            app.diagnostics_scroll = 0;
            Some(output)
        }
        Err(error) => {
            set_notice(app, true, &format!("{label} failed: {error:#}"));
            None
        }
    };
    app.busy = None;
    if refresh_after {
        refresh(terminal, app).await;
    }
    output
}

async fn run_owned_action(
    terminal: &mut TerminalSession,
    app: &mut App,
    label: &'static str,
    args: Vec<String>,
    refresh_after: bool,
) -> Option<String> {
    let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_action(terminal, app, label, &borrowed, refresh_after).await
}

async fn apply_provider_changes(terminal: &mut TerminalSession, app: &mut App) {
    if app.snapshot.providers.is_empty() {
        run_action(
            terminal,
            app,
            "Stopping gateway without providers",
            &["service", "stop"],
            true,
        )
        .await;
        return;
    }
    let restarted = run_action(
        terminal,
        app,
        "Applying provider configuration",
        &["service", "restart"],
        true,
    )
    .await
    .is_some();
    if restarted && app.snapshot.codex_install_mode.is_some() {
        run_action(
            terminal,
            app,
            "Refreshing Codex model catalog",
            &["refresh-codex-catalog"],
            true,
        )
        .await;
    }
}

fn selected_configured_provider_id(app: &App) -> Option<String> {
    let provider = app.selected_provider()?;
    if provider.get("kind").and_then(Value::as_str) == Some("official") {
        return None;
    }
    provider
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn set_notice(app: &mut App, is_error: bool, message: &str) {
    app.notice = message.trim().to_owned();
    app.notice_is_error = is_error;
}

async fn run_json(args: &[&str]) -> anyhow::Result<Value> {
    let output = run_cli(args).await?;
    serde_json::from_str(&output).with_context(|| format!("parse `{}` output", args.join(" ")))
}

async fn run_cli(args: &[&str]) -> anyhow::Result<String> {
    let executable = std::env::current_exe().context("resolve current executable")?;
    let output = Command::new(&executable)
        .arg("--no-tui")
        .args(args)
        .output()
        .await
        .with_context(|| format!("run {} --no-tui {}", executable.display(), args.join(" ")))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        anyhow::bail!(
            "`{}` exited with {}: {}",
            args.join(" "),
            output.status,
            message
        )
    }
    String::from_utf8(output.stdout).context("CLI output is not valid UTF-8")
}

async fn read_event() -> anyhow::Result<Option<Event>> {
    tokio::task::spawn_blocking(|| -> anyhow::Result<Option<Event>> {
        if event::poll(Duration::from_millis(100)).context("poll terminal event")? {
            return event::read().map(Some).context("read terminal event");
        }
        Ok(None)
    })
    .await
    .context("join terminal event reader")?
}

fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = frame.area();
    app.viewport.set(area);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(frame, sections[0], app);
    render_tabs(frame, sections[1], app.page);
    match app.page {
        Page::Dashboard => render_dashboard(frame, sections[2], app),
        Page::Setup => render_setup(frame, sections[2], app),
        Page::Providers => render_providers(frame, sections[2], app),
        Page::Models => render_models(frame, sections[2], app),
        Page::Benchmark => render_benchmark(frame, sections[2], app),
        Page::Integrations => render_integrations(frame, sections[2], app),
        Page::System => render_system(frame, sections[2], app),
        Page::Diagnostics => render_diagnostics(frame, sections[2], app),
    }
    render_footer(frame, sections[3], app);
    if app.help_visible {
        render_help(frame, area);
    }
    if let Some(dialog) = &app.dialog {
        render_dialog(frame, area, dialog, &app.notice, app.notice_is_error);
    }
}

fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let gateway = if app.snapshot.gateway_running() {
        Span::styled(
            " ONLINE ",
            Style::default().fg(Color::Black).bg(Color::Green),
        )
    } else {
        Span::styled(
            " OFFLINE ",
            Style::default().fg(Color::White).bg(Color::Red),
        )
    };
    let title = vec![
        Line::from(vec![
            Span::styled(
                " CODEX ",
                Style::default().fg(Color::Black).bg(Color::Cyan).bold(),
            ),
            Span::styled(
                " MIXIN ",
                Style::default().fg(Color::White).bg(Color::Blue).bold(),
            ),
            Span::styled(
                format!("  v{}  ", env!("CARGO_PKG_VERSION")),
                Style::default().fg(Color::Gray),
            ),
            gateway,
        ]),
        Line::from(Span::styled(
            "  LOCAL AI ROUTING CONTROL DECK",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(title)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .alignment(Alignment::Left),
        area,
    );
}

fn render_tabs(frame: &mut ratatui::Frame<'_>, area: Rect, page: Page) {
    let selected = Page::ALL.iter().position(|item| *item == page).unwrap_or(0);
    let titles = Page::ALL
        .iter()
        .map(|item| Line::from(format!(" {} ", item.title())))
        .collect::<Vec<_>>();
    frame.render_widget(
        Tabs::new(titles)
            .select(selected)
            .style(Style::default().fg(Color::DarkGray))
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )
            .divider(Span::styled("|", Style::default().fg(Color::Blue))),
        area,
    );
}

fn render_setup(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(area);
    let form = &app.setup;
    let provider = &form.provider;
    let required = provider.active_fields();
    let completed = required
        .iter()
        .filter(|field| match field {
            0 => true,
            1 => true,
            2 => !provider.display_name.trim().is_empty(),
            3 => !provider.base_url.trim().is_empty(),
            4 => true,
            5 => !provider.api_key.trim().is_empty(),
            6 => !provider.quota_username.trim().is_empty(),
            7 => !provider.quota_workspace_id.trim().is_empty(),
            8 => !provider.quota_auth_cookie.trim().is_empty(),
            _ => false,
        })
        .count();
    let ratio = completed as f64 / required.len().max(1) as f64;
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(9), Constraint::Length(4)])
        .split(columns[0]);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Connect your first provider",
                Style::default().fg(Color::Cyan).bold(),
            )),
            Line::from(""),
            Line::from("01  Provider and credentials"),
            Line::from("02  Discover and select models"),
            Line::from("03  Start the local gateway"),
            Line::from("04  Connect Codex"),
            Line::from(""),
            Line::from(Span::styled(
                "Enter runs the complete setup inside this UI.",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(
            Block::default()
                .title(" QUICK START ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .wrap(Wrap { trim: true }),
        left[0],
    );
    frame.render_widget(
        Gauge::default()
            .block(
                Block::default()
                    .title(" READY ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black).bold())
            .ratio(ratio)
            .label(format!("{completed}/{} fields", required.len())),
        left[1],
    );

    let provider_id = provider.provider_id();
    let mut lines = vec![
        form_line("Preset", provider.preset(), form.focus == 0, false),
        form_line("Provider ID", &provider_id, form.focus == 1, false),
    ];
    if provider.preset() == "custom" {
        lines.extend([
            form_line(
                "Display name",
                &provider.display_name,
                form.focus == 2,
                false,
            ),
            form_line("Base URL", &provider.base_url, form.focus == 3, false),
            form_line("Website", &provider.website_url, form.focus == 4, false),
        ]);
    }
    lines.push(form_line(
        "API key",
        &provider.api_key,
        form.focus == 5,
        true,
    ));
    if provider.preset() == "baidu-oneapi" {
        lines.push(form_line(
            "Quota user",
            &provider.quota_username,
            form.focus == 6,
            false,
        ));
    } else if provider.preset() == "opencode-go" {
        lines.extend([
            form_line(
                "Workspace ID",
                &provider.quota_workspace_id,
                form.focus == 7,
                false,
            ),
            form_line(
                "Auth cookie",
                &provider.quota_auth_cookie,
                form.focus == 8,
                true,
            ),
        ]);
    }
    lines.extend([
        Line::from(""),
        form_line("Codex mode", form.codex_mode_name(), form.focus == 9, false),
        Line::from(""),
        Line::from(Span::styled(
            "Tab move  Left/Right choose  Enter run  Esc dashboard",
            Style::default().fg(Color::DarkGray),
        )),
    ]);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" SETUP WORKSPACE ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: true }),
        columns[1],
    );
}

fn render_dashboard(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(5)])
        .split(area);
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(36),
            Constraint::Percentage(28),
            Constraint::Percentage(36),
        ])
        .split(sections[0]);

    let gateway = if app.snapshot.gateway_running() {
        Span::styled("RUNNING", Style::default().fg(Color::Green).bold())
    } else {
        Span::styled("STOPPED", Style::default().fg(Color::Red).bold())
    };
    let endpoint = app
        .snapshot
        .status
        .get("endpoint")
        .and_then(Value::as_str)
        .unwrap_or("not available");
    let daemon = app
        .snapshot
        .status
        .get("daemon")
        .and_then(Value::as_str)
        .unwrap_or("not started");
    let configured = if app.snapshot.configured() {
        "yes"
    } else {
        "no"
    };
    let codex_mode = app
        .snapshot
        .codex_install_mode
        .as_deref()
        .unwrap_or("not managed");
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![Span::raw("Gateway     "), gateway]),
            Line::from(format!("Daemon      {daemon}")),
            Line::from(format!("Configured  {configured}")),
            Line::from(format!("Codex mode  {codex_mode}")),
            Line::from(format!("Endpoint    {endpoint}")),
        ])
        .block(
            Block::default()
                .title(" Runtime ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .wrap(Wrap { trim: true }),
        cards[0],
    );

    let healthy = count_provider_status(&app.snapshot.providers, "healthy");
    let degraded = count_provider_status(&app.snapshot.providers, "degraded");
    let disabled = count_provider_status(&app.snapshot.providers, "disabled");
    let selected_models = app
        .snapshot
        .providers
        .iter()
        .filter_map(|provider| provider.get("selected_models").and_then(Value::as_array))
        .map(Vec::len)
        .sum::<usize>();
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Providers    {}", app.snapshot.providers.len())),
            Line::from(vec![
                Span::raw("Healthy      "),
                Span::styled(healthy.to_string(), Style::default().fg(Color::Green)),
            ]),
            Line::from(vec![
                Span::raw("Degraded     "),
                Span::styled(degraded.to_string(), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(format!("Disabled     {disabled}")),
            Line::from(format!("Models       {selected_models} selected")),
        ])
        .block(
            Block::default()
                .title(" Providers ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        cards[1],
    );

    let quota_lines = if app.quota.is_empty() {
        vec![
            Line::from("No quota data"),
            Line::from(Span::styled(
                "Press c to refresh",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        app.quota
            .iter()
            .take(5)
            .map(|quota| {
                let name = value_str(quota, "display_name", "-");
                let currency = value_str(quota, "currency", "");
                let remaining = quota
                    .get("remaining")
                    .and_then(Value::as_f64)
                    .map(|value| format!("{value:.1} {currency}"))
                    .unwrap_or_else(|| "unavailable".to_owned());
                Line::from(vec![
                    Span::styled(format!("{name:<14}"), Style::default().fg(Color::Gray)),
                    Span::styled(remaining, Style::default().fg(Color::Green)),
                ])
            })
            .collect()
    };
    frame.render_widget(
        Paragraph::new(quota_lines)
            .block(
                Block::default()
                    .title(" Quota ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Magenta)),
            )
            .wrap(Wrap { trim: true }),
        cards[2],
    );

    let usage_rows = app
        .snapshot
        .usage
        .iter()
        .skip(app.usage_offset)
        .map(|usage| {
            let cache = usage
                .get("cache_hit_percent")
                .and_then(Value::as_f64)
                .map(|value| format!("{value:.1}%"))
                .unwrap_or_else(|| "-".to_owned());
            let ttft = usage
                .get("average_ttft_ms")
                .and_then(Value::as_f64)
                .map(|value| format!("{value:.0} ms"))
                .unwrap_or_else(|| "-".to_owned());
            let tps = usage
                .get("output_tps")
                .and_then(Value::as_f64)
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| "-".to_owned());
            Row::new(vec![
                value_str(usage, "provider_id", "-").to_owned(),
                value_str(usage, "model_id", "-").to_owned(),
                usage
                    .get("request_count")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "-".to_owned()),
                cache,
                ttft,
                tps,
            ])
        });
    frame.render_widget(
        Table::new(
            usage_rows,
            [
                Constraint::Length(16),
                Constraint::Min(20),
                Constraint::Length(8),
                Constraint::Length(9),
                Constraint::Length(11),
                Constraint::Length(8),
            ],
        )
        .header(
            Row::new([
                "PROVIDER", "MODEL", "REQUESTS", "CACHE", "AVG TTFT", "TOK/S",
            ])
            .style(Style::default().fg(Color::Cyan).bold()),
        )
        .block(
            Block::default()
                .title(" Token usage ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        ),
        sections[1],
    );
}

fn render_providers(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(30), Constraint::Min(36)])
        .split(area);
    let items = app.snapshot.providers.iter().map(|provider| {
        let enabled = provider
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let readiness = value_str(provider, "readiness", "unknown");
        let color = if !enabled && value_str(provider, "kind", "") != "official" {
            Color::DarkGray
        } else if readiness == "healthy" {
            Color::Green
        } else {
            Color::Yellow
        };
        ListItem::new(vec![
            Line::from(Span::styled(
                value_str(provider, "display_name", "-"),
                Style::default().bold(),
            )),
            Line::from(vec![
                Span::styled(format!("{readiness:<10}"), Style::default().fg(color)),
                Span::styled(
                    value_str(provider, "id", "-"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
        ])
    });
    let mut state = ratatui::widgets::ListState::default();
    if !app.snapshot.providers.is_empty() {
        state.select(Some(app.provider_index));
    }
    frame.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
            .highlight_symbol("> ")
            .block(
                Block::default()
                    .title(" Providers ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            ),
        columns[0],
        &mut state,
    );

    let lines = if let Some(provider) = app.selected_provider() {
        let selected = provider
            .get("selected_models")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let cached = provider
            .get("cached_models")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let enabled = provider
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        vec![
            Line::from(Span::styled(
                value_str(provider, "display_name", "-"),
                Style::default().fg(Color::Cyan).bold(),
            )),
            Line::from(""),
            Line::from(format!("ID          {}", value_str(provider, "id", "-"))),
            Line::from(format!(
                "Preset      {}",
                value_str(provider, "preset_id", "official")
            )),
            Line::from(format!(
                "State       {}",
                if enabled { "enabled" } else { "disabled" }
            )),
            Line::from(format!(
                "Readiness   {}",
                value_str(provider, "readiness", "unknown")
            )),
            Line::from(format!(
                "Protocol    {}",
                value_str(provider, "protocol", "-")
            )),
            Line::from(format!("Models      {selected}/{cached} selected")),
            Line::from(format!(
                "Base URL    {}",
                value_str(provider, "base_url", "managed by Codex")
            )),
            Line::from(""),
            Line::from(Span::styled(
                "a add  u edit  D delete  e enable  t test  m discover",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        vec![Line::from("No providers configured. Press a to add one.")]
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Provider details ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .wrap(Wrap { trim: true }),
        columns[1],
    );
}

fn render_models(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(area);
    let provider = app.selected_provider();
    let title = provider
        .map(|provider| value_str(provider, "display_name", "-"))
        .unwrap_or("No provider");
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {title}  "),
                Style::default().fg(Color::Cyan).bold(),
            ),
            Span::styled(
                " [SAVE] ",
                Style::default().fg(Color::Black).bg(Color::Green).bold(),
            ),
            Span::raw("  [ALL]  [NONE]  [DISCOVER]  [PROBE]"),
        ]))
        .block(
            Block::default()
                .title(" MODEL WORKSPACE ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Blue)),
        ),
        sections[0],
    );
    let rows = app.selected_models().into_iter().map(|model| {
        let id = model_id(model);
        let selected = app.model_draft.contains(id);
        let display_name = model
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(id);
        let context = model
            .get("context_window")
            .and_then(Value::as_u64)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_owned());
        Row::new(vec![
            if selected { "[x]" } else { "[ ]" }.to_owned(),
            id.to_owned(),
            display_name.to_owned(),
            context,
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(28),
            Constraint::Min(20),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(["", "MODEL ID", "DISPLAY NAME", "CONTEXT"])
            .style(Style::default().fg(Color::Cyan).bold()),
    )
    .row_highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
    .highlight_symbol("> ")
    .block(
        Block::default()
            .title(format!(" Models: {title} "))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );
    let mut state = TableState::default();
    if !app.selected_models().is_empty() {
        state.select(Some(app.model_index));
    }
    frame.render_stateful_widget(table, sections[1], &mut state);
}

fn render_benchmark(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(6)])
        .split(area);
    let selected_provider = app
        .selected_provider()
        .map(|provider| value_str(provider, "display_name", "all providers"))
        .unwrap_or("all providers");
    let status = app
        .snapshot
        .benchmark
        .as_ref()
        .and_then(|snapshot| snapshot.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("not run");
    let current = app
        .snapshot
        .benchmark
        .as_ref()
        .and_then(|snapshot| snapshot.get("current_model"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Status  ", Style::default().fg(Color::DarkGray)),
                Span::styled(status, benchmark_status_style(status)),
                Span::raw(format!("    Next run  {selected_provider}")),
            ]),
            Line::from(format!(
                "Current {current}    [b / click] RUN    timeout {}s [-/+]    output {} [,/.]",
                app.benchmark_timeout_seconds, app.benchmark_output_tokens
            )),
        ])
        .block(
            Block::default()
                .title(" Model benchmark ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        ),
        sections[0],
    );

    let results = app
        .snapshot
        .benchmark
        .as_ref()
        .and_then(|snapshot| snapshot.get("results"))
        .and_then(Value::as_array);
    let rows = results.into_iter().flatten().map(|result| {
        let ttft = result
            .get("ttft_ms")
            .and_then(Value::as_u64)
            .map(|value| format!("{value} ms"))
            .unwrap_or_else(|| "-".to_owned());
        let tps = result
            .get("tps")
            .and_then(Value::as_f64)
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| "-".to_owned());
        Row::new(vec![
            value_str(result, "provider_name", "-").to_owned(),
            value_str(result, "upstream_model", "-").to_owned(),
            value_str(result, "status", "-").to_owned(),
            ttft,
            tps,
        ])
    });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(18),
                Constraint::Min(24),
                Constraint::Length(11),
                Constraint::Length(12),
                Constraint::Length(9),
            ],
        )
        .header(
            Row::new(["PROVIDER", "MODEL", "STATUS", "TTFT", "TOK/S"])
                .style(Style::default().fg(Color::Cyan).bold()),
        )
        .block(
            Block::default()
                .title(" Results ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        ),
        sections[1],
    );
}

fn render_integrations(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let mode = app
        .snapshot
        .codex_install_mode
        .as_deref()
        .unwrap_or("not managed");
    let cards = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("CODEX  ", Style::default().fg(Color::Cyan).bold()),
                Span::styled(mode.to_owned(), Style::default().fg(Color::Green)),
            ]),
            Line::from(vec![
                action_label(0, "[1] Official", app.integration_index),
                Span::raw("   "),
                action_label(1, "[2] Custom-only", app.integration_index),
                Span::raw("   "),
                action_label(2, "[3] Restore", app.integration_index),
            ]),
        ])
        .block(
            Block::default()
                .title(" CODEX ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .wrap(Wrap { trim: true }),
        cards[0],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "CLAUDE CODE",
                Style::default().fg(Color::Magenta).bold(),
            )),
            Line::from(vec![
                action_label(3, "[4] Install / refresh", app.integration_index),
                Span::raw("       "),
                action_label(4, "[5] Restore", app.integration_index),
            ]),
        ])
        .block(
            Block::default()
                .title(" CLAUDE CODE ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Magenta)),
        )
        .wrap(Wrap { trim: true }),
        cards[1],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "DEEPSEEK HARNESS",
                Style::default().fg(Color::Green).bold(),
            )),
            Line::from(vec![
                action_label(5, "[6] Install / refresh", app.integration_index),
                Span::raw("       "),
                action_label(6, "[7] Remove", app.integration_index),
            ]),
        ])
        .block(
            Block::default()
                .title(" DSH ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Green)),
        )
        .wrap(Wrap { trim: true }),
        cards[2],
    );
}

fn render_system(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    let gateway_state = if app.snapshot.gateway_running() {
        "RUNNING"
    } else {
        "STOPPED"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                gateway_state,
                Style::default().fg(Color::Green).bold(),
            )),
            Line::from("[s] Start/stop    [R] Restart"),
        ])
        .block(
            Block::default()
                .title(" GATEWAY ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(if app.system_index == 0 {
                    Color::Yellow
                } else {
                    Color::Green
                })),
        ),
        top[0],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Installed v{}", env!("CARGO_PKG_VERSION"))),
            Line::from("[u] Check, download, and install latest CLI"),
        ])
        .block(
            Block::default()
                .title(" UPDATE ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(if app.system_index == 1 {
                    Color::Yellow
                } else {
                    Color::Cyan
                })),
        )
        .wrap(Wrap { trim: true }),
        top[1],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("[d] Inspect only    [F] Repair safe issues"),
            Line::from("Repair requires confirmation and shows the full report."),
        ])
        .block(
            Block::default()
                .title(" HEALTH ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(if app.system_index == 2 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                })),
        )
        .wrap(Wrap { trim: true }),
        bottom[0],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("[f] Rebuild Codex model catalog"),
            Line::from("[l] Open the latest 200 gateway log lines"),
        ])
        .block(
            Block::default()
                .title(" MAINTENANCE ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(if app.system_index == 3 {
                    Color::Yellow
                } else {
                    Color::Blue
                })),
        ),
        bottom[1],
    );
}

fn render_dialog(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    dialog: &Dialog,
    notice: &str,
    notice_is_error: bool,
) {
    let popup = centered_rect(area, area.width.min(78), area.height.min(22));
    frame.render_widget(Clear, popup);
    let (title, lines) = match dialog {
        Dialog::ConfirmRemove(id) => (
            " Remove provider ",
            vec![
                Line::from(format!("Delete provider {id}?")),
                Line::from("This removes its credentials and model selection."),
                Line::from(""),
                Line::from(Span::styled(
                    "y confirm    n/Esc cancel",
                    Style::default().fg(Color::Red),
                )),
            ],
        ),
        Dialog::ConfirmOperation(operation) => (
            operation.title(),
            vec![
                Line::from(Span::styled(
                    operation.title(),
                    Style::default().fg(Color::Yellow).bold(),
                )),
                Line::from(""),
                Line::from(operation.description()),
                Line::from(""),
                Line::from(Span::styled(
                    "[y] Confirm    [n] Cancel",
                    Style::default().fg(Color::Red),
                )),
            ],
        ),
        Dialog::AddProvider(form) => {
            let provider_id = form.provider_id();
            let mut lines = vec![
                form_line("Preset", form.preset(), form.focus == 0, false),
                form_line("Provider ID", &provider_id, form.focus == 1, false),
            ];
            if form.preset() == "custom" {
                lines.extend([
                    form_line("Display name", &form.display_name, form.focus == 2, false),
                    form_line("Base URL", &form.base_url, form.focus == 3, false),
                    form_line("Website", &form.website_url, form.focus == 4, false),
                ]);
            }
            lines.push(form_line("API key", &form.api_key, form.focus == 5, true));
            if form.preset() == "baidu-oneapi" {
                lines.push(form_line(
                    "Quota user",
                    &form.quota_username,
                    form.focus == 6,
                    false,
                ));
            } else if form.preset() == "opencode-go" {
                lines.extend([
                    form_line(
                        "Workspace ID",
                        &form.quota_workspace_id,
                        form.focus == 7,
                        false,
                    ),
                    form_line(
                        "Auth cookie",
                        &form.quota_auth_cookie,
                        form.focus == 8,
                        true,
                    ),
                ]);
            }
            lines.extend([
                Line::from(""),
                Line::from("Tab field  Left/Right preset  Ctrl-S/Enter add  Esc cancel"),
            ]);
            (" Add provider ", lines)
        }
        Dialog::EditProvider(form) => (
            " Edit provider ",
            vec![
                Line::from(Span::styled(
                    format!("Provider {}", form.id),
                    Style::default().fg(Color::Cyan),
                )),
                Line::from(""),
                form_line("Display name", &form.display_name, form.focus == 0, false),
                form_line("Base URL", &form.base_url, form.focus == 1, false),
                form_line("Website", &form.website_url, form.focus == 2, false),
                form_line("New API key", &form.api_key, form.focus == 3, true),
                Line::from(""),
                Line::from("Leave API key empty to preserve it."),
                Line::from("Tab field  Ctrl-S/Enter save  Esc cancel"),
            ],
        ),
    };
    let mut content = lines;
    if notice_is_error {
        content.push(Line::from(Span::styled(
            notice,
            Style::default().fg(Color::Red),
        )));
    }
    frame.render_widget(
        Paragraph::new(content)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_diagnostics(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(app.diagnostics.as_str())
            .block(
                Block::default()
                    .title(" Health check output ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .scroll((app.diagnostics_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let notice_color = if app.notice_is_error {
        Color::Red
    } else {
        Color::Green
    };
    let status = if let Some(label) = app.busy {
        Line::from(vec![
            Span::styled("* ", Style::default().fg(Color::Cyan)),
            Span::raw(label),
        ])
    } else {
        Line::from(Span::styled(
            app.notice.as_str(),
            Style::default().fg(notice_color),
        ))
    };
    let keys = match app.page {
        Page::Dashboard => {
            "Up/Down token usage  c quota  s start/stop  r refresh  x doctor  ? help"
        }
        Page::Setup => "Tab field  Left/Right option  Enter setup  Esc dashboard",
        Page::Providers => "a add  u edit  D delete  e enable  t test  m discover  ? help",
        Page::Models => {
            "[ ] provider  Up/Down row  Space/click toggle  a all  n none  s save  d discover"
        }
        Page::Benchmark => "[ ] provider  b/click run  -/+ timeout  ,/. output tokens  r refresh",
        Page::Integrations => "1-3 Codex  4-5 Claude  6-7 DSH  click or press a number",
        Page::System => "s/R gateway  u update  d doctor  F repair  f catalog  l logs",
        Page::Diagnostics => "x doctor  PgUp/PgDn scroll  r refresh  ? help  q quit",
    };
    frame.render_widget(
        Paragraph::new(vec![
            status,
            Line::from(Span::styled(keys, Style::default().fg(Color::DarkGray))),
        ])
        .block(Block::default().borders(Borders::TOP)),
        area,
    );
}

fn render_help(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let width = area.width.min(72);
    let height = area.height.min(18);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("Tab / Left / Right   Change page"),
            Line::from("Mouse click          Tabs, rows, and page actions"),
            Line::from("a / u / D            Add, edit, delete provider"),
            Line::from("e / t / m            Enable, test, discover provider"),
            Line::from("Space / s / p        Toggle, save, probe models"),
            Line::from("b                    Benchmark selected provider"),
            Line::from("1-7                  Install or restore integrations"),
            Line::from("u / F / f / l        Update, repair, catalog, logs"),
            Line::from("r                    Refresh status"),
            Line::from("s / R                Start-stop / restart gateway"),
            Line::from("x                    Run quick doctor"),
            Line::from("q                    Quit"),
            Line::from(""),
            Line::from(Span::styled(
                "Press any key to close",
                Style::default().fg(Color::Cyan),
            )),
        ])
        .block(
            Block::default()
                .title(" Keyboard shortcuts ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: true }),
        popup,
    );
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn form_line(label: &str, value: &str, focused: bool, secret: bool) -> Line<'static> {
    let shown = if secret && !value.is_empty() {
        "*".repeat(value.chars().count())
    } else if value.is_empty() {
        "<empty>".to_owned()
    } else {
        value.to_owned()
    };
    let style = if focused {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(format!("{label:<15}"), Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" {shown} "), style),
    ])
}

fn action_label(index: usize, label: &'static str, selected: usize) -> Span<'static> {
    if index == selected {
        Span::styled(
            format!(" {label} "),
            Style::default().fg(Color::Black).bg(Color::Cyan).bold(),
        )
    } else {
        Span::styled(label, Style::default().fg(Color::Gray))
    }
}

fn model_id(model: &Value) -> &str {
    model
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| model.as_str())
        .unwrap_or("-")
}

fn benchmark_status_style(status: &str) -> Style {
    match status {
        "completed" => Style::default().fg(Color::Green).bold(),
        "running" => Style::default().fg(Color::Cyan).bold(),
        "failed" | "interrupted" => Style::default().fg(Color::Red).bold(),
        _ => Style::default().fg(Color::DarkGray),
    }
}

fn count_provider_status(providers: &[Value], status: &str) -> usize {
    providers
        .iter()
        .filter(|provider| provider.get("readiness").and_then(Value::as_str) == Some(status))
        .count()
}

fn value_str<'a>(value: &'a Value, key: &str, default: &'a str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or(default)
}

fn pretty_json_or_text(output: &str) -> String {
    serde_json::from_str::<Value>(output)
        .and_then(|value| serde_json::to_string_pretty(&value))
        .unwrap_or_else(|_| output.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_handles_unconfigured_status() {
        let snapshot = Snapshot {
            status: serde_json::json!({"configured": false}),
            providers: Vec::new(),
            codex_install_mode: None,
            benchmark: None,
            usage: Vec::new(),
            refreshed_at: Instant::now(),
        };
        assert!(!snapshot.configured());
        assert!(!snapshot.gateway_running());
    }

    #[test]
    fn provider_status_count_uses_readiness() {
        let providers = vec![
            serde_json::json!({"readiness": "healthy"}),
            serde_json::json!({"readiness": "healthy"}),
            serde_json::json!({"readiness": "degraded"}),
        ];
        assert_eq!(count_provider_status(&providers, "healthy"), 2);
        assert_eq!(count_provider_status(&providers, "degraded"), 1);
    }

    #[test]
    fn pretty_json_keeps_plain_command_output() {
        assert_eq!(pretty_json_or_text("done\n"), "done");
        assert!(pretty_json_or_text("{\"ok\":true}").contains("\"ok\": true"));
    }

    #[test]
    fn custom_provider_form_builds_complete_cli_arguments() {
        let form = AddProviderForm {
            preset_index: 4,
            display_name: "Community API".to_owned(),
            base_url: "https://example.com/v1".to_owned(),
            api_key: "secret".to_owned(),
            ..AddProviderForm::default()
        };
        let args = form.args().unwrap();
        assert!(args.windows(2).any(|pair| pair == ["--preset", "custom"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--base-url", "https://example.com/v1"])
        );
        assert!(args.windows(2).any(|pair| pair == ["--key", "secret"]));
    }

    #[test]
    fn inactive_provider_fields_are_not_submitted() {
        let form = AddProviderForm {
            preset_index: 1,
            display_name: "stale custom name".to_owned(),
            base_url: "https://stale.example".to_owned(),
            api_key: "secret".to_owned(),
            ..AddProviderForm::default()
        };
        let args = form.args().unwrap();
        assert!(!args.iter().any(|argument| argument == "--base-url"));
        assert!(!args.iter().any(|argument| argument == "--display-name"));
    }

    #[test]
    fn unconfigured_launch_opens_the_setup_workspace() {
        let snapshot = Snapshot {
            status: serde_json::json!({"configured": false}),
            providers: Vec::new(),
            codex_install_mode: None,
            benchmark: None,
            usage: Vec::new(),
            refreshed_at: Instant::now(),
        };
        let app = App::new(snapshot, StartPage::Dashboard);
        assert_eq!(app.page, Page::Setup);
    }

    #[test]
    fn mouse_selects_tabs_and_integration_actions() {
        let snapshot = Snapshot {
            status: serde_json::json!({"configured": true, "gateway": "stopped"}),
            providers: Vec::new(),
            codex_install_mode: None,
            benchmark: None,
            usage: Vec::new(),
            refreshed_at: Instant::now(),
        };
        let mut app = App::new(snapshot, StartPage::Dashboard);
        app.viewport.set(Rect::new(0, 0, 100, 30));
        assert_eq!(
            handle_mouse_event(&mut app, MouseEventKind::Down(MouseButton::Left), 8, 5),
            Action::None
        );
        assert_eq!(app.page, Page::Setup);
        app.page = Page::Integrations;
        assert_eq!(
            handle_mouse_event(&mut app, MouseEventKind::Down(MouseButton::Left), 90, 8),
            Action::ConfirmUninstallCodex
        );
    }

    #[test]
    fn setup_workspace_renders_at_eighty_by_twenty_four() {
        let snapshot = Snapshot {
            status: serde_json::json!({"configured": false}),
            providers: Vec::new(),
            codex_install_mode: None,
            benchmark: None,
            usage: Vec::new(),
            refreshed_at: Instant::now(),
        };
        let app = App::new(snapshot, StartPage::Setup);
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("SETUP WORKSPACE"));
        assert!(rendered.contains("LOCAL AI ROUTING CONTROL DECK"));
        assert!(rendered.contains(" Logs "));
    }
}
