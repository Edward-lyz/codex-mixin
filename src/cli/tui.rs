use std::io::{self, IsTerminal, Stdout};
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
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
    Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use serde_json::Value;
use tokio::process::Command;

const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Page {
    Dashboard,
    Providers,
    Connections,
    Diagnostics,
}

impl Page {
    const ALL: [Self; 4] = [
        Self::Dashboard,
        Self::Providers,
        Self::Connections,
        Self::Diagnostics,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Providers => "Providers",
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
        Ok(Self {
            status,
            providers,
            codex_install_mode,
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
}

struct App {
    page: Page,
    snapshot: Snapshot,
    provider_index: usize,
    notice: String,
    notice_is_error: bool,
    diagnostics: String,
    diagnostics_scroll: u16,
    help_visible: bool,
    busy: Option<&'static str>,
}

impl App {
    fn new(snapshot: Snapshot) -> Self {
        Self {
            page: Page::Dashboard,
            snapshot,
            provider_index: 0,
            notice: "Ready".to_owned(),
            notice_is_error: false,
            diagnostics: "Press x to run a quick health check.".to_owned(),
            diagnostics_scroll: 0,
            help_visible: false,
            busy: None,
        }
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
    RunDoctor,
}

pub(super) async fn run() -> anyhow::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("the full-screen UI requires a terminal; use --no-tui for plain output")
    }

    let snapshot = Snapshot::load().await?;
    let mut app = App::new(snapshot);
    let mut terminal = TerminalSession::enter()?;
    terminal.draw(&app)?;

    loop {
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
                }
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
        KeyCode::Up if app.page == Page::Providers => {
            app.provider_index = app.provider_index.saturating_sub(1);
            Action::None
        }
        KeyCode::Down if app.page == Page::Providers => {
            app.provider_index =
                (app.provider_index + 1).min(app.snapshot.providers.len().saturating_sub(1));
            Action::None
        }
        KeyCode::PageUp if app.page == Page::Diagnostics => {
            app.diagnostics_scroll = app.diagnostics_scroll.saturating_sub(10);
            Action::None
        }
        KeyCode::PageDown if app.page == Page::Diagnostics => {
            app.diagnostics_scroll = app.diagnostics_scroll.saturating_add(10);
            Action::None
        }
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Char('s') => Action::ToggleGateway,
        KeyCode::Char('R') => Action::RestartGateway,
        KeyCode::Char('e') if app.page == Page::Providers => Action::ToggleProvider,
        KeyCode::Char('t') if app.page == Page::Providers => Action::TestProvider,
        KeyCode::Char('m') if app.page == Page::Providers => Action::DiscoverModels,
        KeyCode::Char('x') => Action::RunDoctor,
        _ => Action::None,
    }
}

async fn refresh(terminal: &mut TerminalSession, app: &mut App) {
    app.busy = Some("Refreshing status");
    let _ = terminal.draw(app);
    match Snapshot::load().await {
        Ok(snapshot) => {
            app.snapshot = snapshot;
            app.clamp_provider_index();
            set_notice(app, false, "Status refreshed.");
        }
        Err(error) => set_notice(app, true, &format!("Refresh failed: {error:#}")),
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
        Page::Connections => render_connections(frame, sections[2], app),
        Page::Diagnostics => render_diagnostics(frame, sections[2], app),
    }
    render_footer(frame, sections[3], app);
    if app.help_visible {
        render_help(frame, area);
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
    let horizontal = area.width >= 88;
    let direction = if horizontal {
        Direction::Horizontal
    } else {
        Direction::Vertical
    };
    let cards = Layout::default()
        .direction(direction)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

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
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![Span::raw("Gateway     "), gateway]),
            Line::from(format!("Daemon      {daemon}")),
            Line::from(format!("Configured  {configured}")),
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
}

fn render_providers(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let rows = app.snapshot.providers.iter().map(|provider| {
        let state = if provider
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "enabled"
        } else {
            "disabled"
        };
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
        let readiness = value_str(provider, "readiness", "unknown");
        let readiness_style = match readiness {
            "healthy" => Style::default().fg(Color::Green),
            "degraded" => Style::default().fg(Color::Yellow),
            _ => Style::default().fg(Color::Red),
        };
        Row::new(vec![
            Cell::from(value_str(provider, "id", "-")),
            Cell::from(value_str(provider, "display_name", "-")),
            Cell::from(state),
            Cell::from(value_str(provider, "protocol", "-")),
            Cell::from(format!("{selected}/{cached}")),
            Cell::from(readiness).style(readiness_style),
        ])
    });
    let header = Row::new(["ID", "NAME", "STATE", "PROTOCOL", "MODELS", "STATUS"])
        .style(Style::default().fg(Color::Cyan).bold())
        .bottom_margin(1);
    let widths = if area.width < 100 {
        [
            Constraint::Length(14),
            Constraint::Min(12),
            Constraint::Length(9),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(9),
        ]
    } else {
        [
            Constraint::Length(18),
            Constraint::Min(18),
            Constraint::Length(10),
            Constraint::Length(19),
            Constraint::Length(9),
            Constraint::Length(10),
        ]
    };
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).bold())
        .highlight_symbol("> ")
        .block(
            Block::default()
                .title(" Provider routing ")
                .borders(Borders::ALL),
        );
    let mut state = TableState::default();
    if !app.snapshot.providers.is_empty() {
        state.select(Some(app.provider_index));
    }
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_connections(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let mode = app
        .snapshot
        .codex_install_mode
        .as_deref()
        .unwrap_or("not managed");
    let official = app
        .snapshot
        .providers
        .iter()
        .any(|provider| provider.get("kind").and_then(Value::as_str) == Some("official"));
    let items = vec![
        ListItem::new(Line::from(vec![
            Span::styled("Codex       ", Style::default().fg(Color::Cyan).bold()),
            Span::raw(mode),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("OpenAI      ", Style::default().fg(Color::Cyan).bold()),
            Span::styled(
                if official { "available" } else { "not routed" },
                Style::default().fg(if official {
                    Color::Green
                } else {
                    Color::DarkGray
                }),
            ),
        ])),
        ListItem::new(""),
        ListItem::new("Connection setup remains available through explicit commands:"),
        ListItem::new("  codex-mixin connect codex ..."),
        ListItem::new("  codex-mixin connect claude ..."),
        ListItem::new("  codex-mixin connect dsh ..."),
    ];
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(" Client connections ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        ),
        area,
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
        Page::Providers => "Up/Down select  e enable  t test  m models  r refresh  ? help  q quit",
        Page::Diagnostics => "x doctor  PgUp/PgDn scroll  r refresh  ? help  q quit",
        _ => "Tab/Left/Right page  s start/stop  R restart  r refresh  x doctor  ? help  q quit",
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
            Line::from("Up / Down            Select provider"),
            Line::from("r                    Refresh status"),
            Line::from("s / R                Start-stop / restart gateway"),
            Line::from("e / t / m            Enable, test, refresh provider models"),
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
}
