use std::collections::HashSet;
use std::io::{self, IsTerminal, Stdout};
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
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
    Block, Borders, Clear, List, ListItem, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use serde_json::Value;
use tokio::process::Command;

const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Page {
    Dashboard,
    Providers,
    Models,
    Benchmark,
    Connections,
    Diagnostics,
}

impl Page {
    const ALL: [Self; 6] = [
        Self::Dashboard,
        Self::Providers,
        Self::Models,
        Self::Benchmark,
        Self::Connections,
        Self::Diagnostics,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Providers => "Providers",
            Self::Models => "Models",
            Self::Benchmark => "Benchmark",
            Self::Connections => "Connections",
            Self::Diagnostics => "Diagnostics",
        }
    }
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
}

impl App {
    fn new(snapshot: Snapshot) -> Self {
        let mut app = Self {
            page: Page::Dashboard,
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
    StartBenchmark,
    ConnectCodexOfficial,
    ConnectCodexCustom,
    ConnectClaude,
    ConnectDsh,
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
pub(super) async fn run() -> anyhow::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("the full-screen UI requires a terminal; use --no-tui for plain output")
    }

    let snapshot = Snapshot::load().await?;
    let mut app = App::new(snapshot);
    let mut terminal = TerminalSession::enter()?;
    terminal.draw(&app)?;

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
                        run_action(
                            &mut terminal,
                            &mut app,
                            "Updating provider",
                            &["providers", operation, &id],
                            true,
                        )
                        .await;
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
                    run_owned_action(
                        &mut terminal,
                        &mut app,
                        "Saving model selection",
                        args,
                        true,
                    )
                    .await;
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
                run_owned_action(
                    &mut terminal,
                    &mut app,
                    "Removing provider",
                    vec!["providers".to_owned(), "remove".to_owned(), id],
                    true,
                )
                .await;
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
                        run_owned_action(&mut terminal, &mut app, label, args, true).await;
                        app.load_model_draft();
                    }
                    Err(error) => set_notice(&mut app, true, &error.to_string()),
                }
            }
            Action::StartBenchmark => {
                let mut args = vec![
                    "benchmark".to_owned(),
                    "start".to_owned(),
                    "--timeout-seconds".to_owned(),
                    "120".to_owned(),
                    "--target-output-tokens".to_owned(),
                    "100".to_owned(),
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
    let Event::Key(key) = event else {
        return Action::None;
    };
    if key.kind != KeyEventKind::Press {
        return Action::None;
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
        KeyCode::Tab | KeyCode::Right => {
            app.next_page(1);
            Action::None
        }
        KeyCode::BackTab | KeyCode::Left => {
            app.next_page(-1);
            Action::None
        }
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Char('s') if app.page != Page::Models => Action::ToggleGateway,
        KeyCode::Char('R') => Action::RestartGateway,
        KeyCode::Char('x') => Action::RunDoctor,
        code => handle_page_event(app, code),
    }
}

fn handle_page_event(app: &mut App, code: KeyCode) -> Action {
    match app.page {
        Page::Providers => handle_provider_event(app, code),
        Page::Models => handle_model_event(app, code),
        Page::Benchmark => handle_benchmark_event(app, code),
        Page::Connections => match code {
            KeyCode::Char('1') => Action::ConnectCodexOfficial,
            KeyCode::Char('2') => Action::ConnectCodexCustom,
            KeyCode::Char('3') => Action::ConnectClaude,
            KeyCode::Char('4') => Action::ConnectDsh,
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
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(frame, sections[0]);
    render_tabs(frame, sections[1], app.page);
    match app.page {
        Page::Dashboard => render_dashboard(frame, sections[2], app),
        Page::Providers => render_providers(frame, sections[2], app),
        Page::Models => render_models(frame, sections[2], app),
        Page::Benchmark => render_benchmark(frame, sections[2], app),
        Page::Connections => render_connections(frame, sections[2], app),
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

fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let title = Line::from(vec![
        Span::styled(" CODEX ", Style::default().fg(Color::Black).bg(Color::Cyan)),
        Span::styled(" MIXIN ", Style::default().fg(Color::Black).bg(Color::Blue)),
        Span::styled(
            format!("  Control Center  v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(Color::Gray),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(title)
            .block(Block::default().borders(Borders::BOTTOM))
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
            .divider("|"),
        area,
    );
}

fn render_dashboard(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(5)])
        .split(area);
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
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
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        cards[1],
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
                .borders(Borders::ALL),
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
            .block(Block::default().title(" Providers ").borders(Borders::ALL)),
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
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: true }),
        columns[1],
    );
}

fn render_models(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let provider = app.selected_provider();
    let title = provider
        .map(|provider| value_str(provider, "display_name", "-"))
        .unwrap_or("No provider");
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
            .borders(Borders::ALL),
    );
    let mut state = TableState::default();
    if !app.selected_models().is_empty() {
        state.select(Some(app.model_index));
    }
    frame.render_stateful_widget(table, area, &mut state);
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
                "Current {current}    b start 120s / 100 output tokens"
            )),
        ])
        .block(
            Block::default()
                .title(" Model benchmark ")
                .borders(Borders::ALL),
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
        .block(Block::default().title(" Results ").borders(Borders::ALL)),
        sections[1],
    );
}

fn render_connections(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let mode = app
        .snapshot
        .codex_install_mode
        .as_deref()
        .unwrap_or("not managed");
    let cards = Layout::default()
        .direction(if area.width >= 90 {
            Direction::Horizontal
        } else {
            Direction::Vertical
        })
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Codex",
                Style::default().fg(Color::Cyan).bold(),
            )),
            Line::from(format!("Current mode  {mode}")),
            Line::from(""),
            Line::from("1  Install official account mode"),
            Line::from("2  Install custom-only mode"),
            Line::from(""),
            Line::from("A new Codex session is required after changing mode."),
        ])
        .block(
            Block::default()
                .title(" Codex integration ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .wrap(Wrap { trim: true }),
        cards[0],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Developer clients",
                Style::default().fg(Color::Cyan).bold(),
            )),
            Line::from(""),
            Line::from("3  Install Claude Code connection"),
            Line::from("4  Install DeepSeek Harness connection"),
            Line::from(""),
            Line::from("Existing settings are backed up by the CLI before mutation."),
        ])
        .block(
            Block::default()
                .title(" Other clients ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: true }),
        cards[1],
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
        Page::Dashboard => "Up/Down token usage  s start/stop  r refresh  x doctor  ? help",
        Page::Providers => "a add  u edit  D delete  e enable  t test  m discover  ? help",
        Page::Models => "[ ] provider  Up/Down model  Space toggle  s save  d discover  p probe",
        Page::Benchmark => "[ ] provider  b start benchmark  r refresh  ? help  q quit",
        Page::Connections => "1 Codex official  2 Codex custom  3 Claude  4 DSH  ? help",
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
            Line::from("a / u / D            Add, edit, delete provider"),
            Line::from("e / t / m            Enable, test, discover provider"),
            Line::from("Space / s / p        Toggle, save, probe models"),
            Line::from("b                    Benchmark selected provider"),
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
}
