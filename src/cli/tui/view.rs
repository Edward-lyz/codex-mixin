use super::*;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Gauge, List, ListItem, Paragraph, Row, Table, TableState,
    Tabs, Wrap,
};
use serde_json::Value;

pub(super) fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = frame.area();
    app.viewport.set(area);
    if app.busy.is_some() {
        render_busy(frame, area, app);
        return;
    }
    if let Some(dialog @ (Dialog::AddProvider(_) | Dialog::EditProvider(_))) = &app.dialog {
        render_provider_editor(frame, area, app, dialog);
        return;
    }
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
        Page::Fusion => render_fusion(frame, sections[2], app),
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

pub(super) fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let gateway = if app.snapshot.gateway_running() {
        Span::styled(
            " \u{25cf} ONLINE ",
            Style::default().fg(Color::Black).bg(Color::Green),
        )
    } else {
        Span::styled(
            " \u{25cb} OFFLINE ",
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

pub(super) fn render_tabs(frame: &mut ratatui::Frame<'_>, area: Rect, page: Page) {
    let selected = Page::ALL.iter().position(|item| *item == page).unwrap_or(0);
    let compact_tabs = area.width < 110;
    let titles = Page::ALL
        .iter()
        .map(|item| Line::from(format!(" {} ", item.tab_title(compact_tabs))))
        .collect::<Vec<_>>();
    frame.render_widget(
        Tabs::new(titles)
            .select(selected)
            .padding("", "")
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

pub(super) fn render_setup(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
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
    } else if is_aws_bedrock(provider.preset()) {
        lines.extend([
            form_line("AWS region", &provider.aws_region, form.focus == 13, false),
            form_line(
                "Access key ID",
                &provider.aws_access_key_id,
                form.focus == 14,
                true,
            ),
            form_line(
                "Secret access key",
                &provider.aws_secret_access_key,
                form.focus == 15,
                true,
            ),
            form_line(
                "Session token",
                &provider.aws_session_token,
                form.focus == 16,
                true,
            ),
        ]);
    } else {
        lines.push(form_line(
            "API key",
            &provider.api_key,
            form.focus == 5,
            true,
        ));
    }
    if provider.preset() == "baidu-oneapi" {
        lines.extend([
            form_line(
                "Quota user",
                &provider.quota_username,
                form.focus == 6,
                false,
            ),
            form_line(
                "DUCX auth",
                provider.baidu_auth_bridge_name(),
                form.focus == 9,
                false,
            ),
            form_line(
                "Code report",
                bool_name(provider.baidu_code_report),
                form.focus == 10,
                false,
            ),
        ]);
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
        form_line(
            "Image path",
            &provider.image_generation_path,
            form.focus == 12,
            false,
        ),
        form_line(
            "Aux upstream",
            bool_name(provider.auxiliary_model_upstream),
            form.focus == 11,
            false,
        ),
    ]);
    lines.extend([
        Line::from(""),
        form_line(
            "Codex mode",
            form.codex_mode_name(),
            form.focus == 17,
            false,
        ),
        Line::from(""),
        Line::from(Span::styled(
            "Up/Down field  Left/Right choose  Enter run  Tab workspace",
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

pub(super) fn render_dashboard(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
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
        Span::styled("\u{25cf} RUNNING", Style::default().fg(Color::Green).bold())
    } else {
        Span::styled("\u{25cb} STOPPED", Style::default().fg(Color::Red).bold())
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
                .title(" \u{25cf} Runtime ")
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
                .title(" \u{25c6} Providers ")
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
                    .title(" \u{25c8} Quota ")
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
                .title(format!(
                    " \u{3a3} Token usage  {}  selected {} ",
                    USAGE_RANGE_LABELS.join("  "),
                    USAGE_RANGE_LABELS[app.usage_range]
                ))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        ),
        sections[1],
    );
}

pub(super) fn render_providers(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
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
                Span::styled(
                    format!("\u{25cf} {readiness:<8}"),
                    Style::default().fg(color),
                ),
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
            .highlight_symbol("\u{25b8} ")
            .block(
                Block::default()
                    .title(" \u{25c6} Providers ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            ),
        columns[0],
        &mut state,
    );

    let details = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(3)])
        .split(columns[1]);
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
        let mut lines = vec![
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
        ];
        if value_str(provider, "kind", "") == "configured" {
            let auxiliary = provider
                .get("auxiliary_model_upstream")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            lines.extend([
                Line::from(format!(
                    "Aux route   {}",
                    if auxiliary {
                        "selected"
                    } else {
                        "not selected"
                    }
                )),
                Line::from(format!(
                    "Image API   {}",
                    value_str(provider, "image_generation_path", "not configured")
                )),
            ]);
            if value_str(provider, "preset_id", "") == "baidu-oneapi" {
                lines.extend([
                    Line::from(format!(
                        "DUCX auth   {}",
                        value_str(provider, "baidu_auth_bridge", "disabled")
                    )),
                    Line::from(format!(
                        "Code report {}",
                        bool_name(
                            provider
                                .get("baidu_code_report")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                        )
                    )),
                ]);
            }
            if let Some(issues) = provider.get("readiness_issues").and_then(Value::as_array)
                && !issues.is_empty()
            {
                lines.push(Line::from(Span::styled(
                    format!(
                        "Issues      {}",
                        issues
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                    Style::default().fg(Color::Yellow),
                )));
            }
            let new_models = provider
                .get("new_models")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let unavailable = provider
                .get("unavailable_selected_models")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            lines.push(Line::from(format!(
                "Catalog     {new_models} new, {unavailable} unavailable"
            )));
            if let Some(error) = provider
                .get("last_model_refresh_error")
                .and_then(Value::as_str)
            {
                lines.push(Line::from(Span::styled(
                    format!("Refresh     {error}"),
                    Style::default().fg(Color::Red),
                )));
            }
        }
        lines
    } else {
        vec![Line::from("No providers configured. Press a to add one.")]
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" \u{25c7} Provider details ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .wrap(Wrap { trim: true }),
        details[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            PROVIDER_ACTION_LABELS.join("  "),
            Style::default().fg(Color::Cyan),
        )))
        .block(
            Block::default()
                .title(" Actions ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        ),
        details[1],
    );
}

pub(super) fn render_models(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(5)])
        .split(area);
    let provider = app.selected_provider();
    let title = provider
        .map(|provider| value_str(provider, "display_name", "-"))
        .unwrap_or("No provider");
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!("Provider  {title}"),
                Style::default().fg(Color::Cyan).bold(),
            )),
            Line::from(vec![
                Span::styled(
                    MODEL_ACTION_LABELS[0],
                    Style::default().fg(Color::Black).bg(Color::Green).bold(),
                ),
                Span::raw(format!("  {}", MODEL_ACTION_LABELS[1..].join("  "))),
            ]),
        ])
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

pub(super) fn render_benchmark(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
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

pub(super) fn render_fusion(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    let models = fusion_models(&app.snapshot.models);
    let rows = models.iter().map(|model| {
        let id = value_str(model, "id", "-");
        Row::new([
            if app.fusion.panel_models.contains(id) {
                "[x]"
            } else {
                "[ ]"
            },
            if app.fusion.judge_model == id {
                "J"
            } else {
                ""
            },
            if app.fusion.final_model == id {
                "F"
            } else {
                ""
            },
            id,
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(20),
        ],
    )
    .header(Row::new(["P", "J", "F", "MODEL"]).style(Style::default().fg(Color::Cyan).bold()))
    .row_highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
    .highlight_symbol("> ")
    .block(
        Block::default()
            .title(" Panel / Judge / Final models ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );
    let mut state = TableState::default();
    if !models.is_empty() {
        state.select(Some(app.fusion.model_index));
    }
    frame.render_stateful_widget(table, columns[0], &mut state);

    let profile_style = if app.fusion.editing_profile_id {
        Style::default().fg(Color::Black).bg(Color::Cyan).bold()
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!("Profile       {}", app.fusion.profile_id),
                profile_style,
            )),
            Line::from(""),
            Line::from(format!(
                "Panels        {} / 8",
                app.fusion.panel_models.len()
            )),
            Line::from(format!("Judge         {}", app.fusion.judge_model)),
            Line::from(format!("Final         {}", app.fusion.final_model)),
            Line::from(format!("Min success   {}", app.fusion.min_successful)),
            Line::from(format!("Timeout       {} ms", app.fusion.timeout_ms)),
            Line::from(format!(
                "Intermediate  {}",
                bool_name(app.fusion.show_intermediate_results)
            )),
            Line::from(format!(
                "Panel tools   {}",
                bool_name(app.fusion.panel_tools_enabled)
            )),
            Line::from(""),
            Line::from(Span::styled(
                "[ SAVE ]  [ DISABLE ]",
                Style::default().fg(Color::Cyan).bold(),
            )),
            Line::from(""),
            Line::from("Space Panel  j Judge  f Final"),
            Line::from("e Profile  -/+ Min  Left/Right timeout"),
            Line::from("i Intermediate  t Tools"),
        ])
        .block(
            Block::default()
                .title(" Fusion orchestration ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .wrap(Wrap { trim: true }),
        columns[1],
    );
}

pub(super) fn render_integrations(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let mode = app
        .snapshot
        .codex_install_mode
        .as_deref()
        .unwrap_or("not managed");
    let cards = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
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
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "OPENCODE",
                Style::default().fg(Color::Cyan).bold(),
            )),
            Line::from(vec![
                action_label(7, "[8] Install / refresh", app.integration_index),
                Span::raw("       "),
                action_label(8, "[9] Remove", app.integration_index),
            ]),
        ])
        .block(
            Block::default()
                .title(" OPENCODE ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: true }),
        cards[3],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "PI CODING AGENT",
                Style::default().fg(Color::Yellow).bold(),
            )),
            Line::from(vec![
                action_label(9, "[p] Install / refresh", app.integration_index),
                Span::raw("       "),
                action_label(10, "[P] Remove", app.integration_index),
            ]),
        ])
        .block(
            Block::default()
                .title(" PI ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: true }),
        cards[4],
    );
}

pub(super) fn render_system(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
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

pub(super) fn render_provider_editor(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    dialog: &Dialog,
) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area);
    render_header(frame, sections[0], app);
    let operation = if matches!(dialog, Dialog::AddProvider(_)) {
        "Add provider"
    } else {
        "Edit provider"
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " \u{2190} Providers ",
                Style::default().fg(Color::Cyan).bold(),
            ),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled(format!(" {operation}"), Style::default().fg(Color::White)),
        ]))
        .block(Block::default().borders(Borders::BOTTOM)),
        sections[1],
    );
    let (guide_area, form_area) = provider_editor_columns(sections[2]);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "\u{25c6} PROVIDER WORKSPACE",
                Style::default().fg(Color::Blue).bold(),
            )),
            Line::from(""),
            Line::from("Configure routing, credentials, authentication, and upstream behavior."),
            Line::from(""),
            Line::from(Span::styled(
                "Keys",
                Style::default().fg(Color::Cyan).bold(),
            )),
            Line::from("\u{2191}/\u{2193} or Tab   Move"),
            Line::from("\u{2190}/\u{2192} or Space Choose"),
            Line::from("Enter         Save"),
            Line::from("Esc           Back"),
        ])
        .block(
            Block::default()
                .title(" GUIDE ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .wrap(Wrap { trim: true }),
        guide_area,
    );
    render_dialog(frame, form_area, dialog, &app.notice, app.notice_is_error);
    frame.render_widget(
        Paragraph::new(vec![Line::from(Span::styled(
            " [ENTER] Save   [ESC] Back   Mouse and keyboard enabled",
            Style::default().fg(Color::DarkGray),
        ))])
        .block(Block::default().borders(Borders::TOP)),
        sections[3],
    );
}

pub(super) fn provider_editor_columns(area: Rect) -> (Rect, Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);
    (columns[0], columns[1])
}

pub(super) fn provider_editor_form_area(area: Rect) -> Rect {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area);
    provider_editor_columns(sections[2]).1
}

pub(super) fn render_dialog(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    dialog: &Dialog,
    notice: &str,
    notice_is_error: bool,
) {
    let popup = if matches!(dialog, Dialog::AddProvider(_) | Dialog::EditProvider(_)) {
        area
    } else {
        dialog_popup(area)
    };
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
        Dialog::ConfirmDisableFusion(id) => (
            " Disable Fusion ",
            vec![
                Line::from(format!("Disable Fusion profile {id}?")),
                Line::from("This removes its virtual model from the Codex catalog."),
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
            } else if is_aws_bedrock(form.preset()) {
                lines.extend([
                    form_line("AWS region", &form.aws_region, form.focus == 13, false),
                    form_line(
                        "Access key ID",
                        &form.aws_access_key_id,
                        form.focus == 14,
                        true,
                    ),
                    form_line(
                        "Secret access key",
                        &form.aws_secret_access_key,
                        form.focus == 15,
                        true,
                    ),
                    form_line(
                        "Session token",
                        &form.aws_session_token,
                        form.focus == 16,
                        true,
                    ),
                ]);
            } else {
                lines.push(form_line("API key", &form.api_key, form.focus == 5, true));
            }
            if form.preset() == "baidu-oneapi" {
                lines.extend([
                    form_line("Quota user", &form.quota_username, form.focus == 6, false),
                    form_line(
                        "DUCX auth",
                        form.baidu_auth_bridge_name(),
                        form.focus == 9,
                        false,
                    ),
                    form_line(
                        "Code report",
                        bool_name(form.baidu_code_report),
                        form.focus == 10,
                        false,
                    ),
                ]);
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
                form_line(
                    "Image path",
                    &form.image_generation_path,
                    form.focus == 12,
                    false,
                ),
                form_line(
                    "Aux upstream",
                    bool_name(form.auxiliary_model_upstream),
                    form.focus == 11,
                    false,
                ),
                Line::from(""),
                Line::from("[ ADD ]  [ CANCEL ]   Tab/Up/Down field  Left/Right/Space choose"),
            ]);
            (" \u{2295} ADD PROVIDER ", lines)
        }
        Dialog::EditProvider(form) => {
            let mut lines = vec![
                Line::from(Span::styled(
                    format!("Provider {}", form.id),
                    Style::default().fg(Color::Cyan),
                )),
                Line::from(""),
            ];
            if form.preset == "custom" {
                lines.extend([
                    form_line("Display name", &form.display_name, form.focus == 0, false),
                    form_line("Base URL", &form.base_url, form.focus == 1, false),
                    form_line("Website", &form.website_url, form.focus == 2, false),
                ]);
            } else if is_aws_bedrock(&form.preset) {
                lines.extend([
                    form_line("AWS region", &form.aws_region, form.focus == 13, false),
                    form_line(
                        "New access key ID",
                        &form.aws_access_key_id,
                        form.focus == 14,
                        true,
                    ),
                    form_line(
                        "New secret access key",
                        &form.aws_secret_access_key,
                        form.focus == 15,
                        true,
                    ),
                    form_line(
                        "New session token",
                        &form.aws_session_token,
                        form.focus == 16,
                        true,
                    ),
                    form_line(
                        "Clear session token",
                        bool_name(form.clear_aws_session_token),
                        form.focus == 17,
                        false,
                    ),
                    form_line(
                        "Clear AWS credentials",
                        bool_name(form.clear_aws_credentials),
                        form.focus == 18,
                        false,
                    ),
                ]);
            }
            lines.push(form_line(
                "Image path",
                &form.image_generation_path,
                form.focus == 3,
                false,
            ));
            if !is_aws_bedrock(&form.preset) {
                lines.extend([
                    form_line("New API key", &form.api_key, form.focus == 4, true),
                    form_line(
                        "Clear API key",
                        bool_name(form.clear_key),
                        form.focus == 5,
                        false,
                    ),
                ]);
            }
            if form.preset == "baidu-oneapi" {
                lines.extend([
                    form_line("Quota user", &form.quota_username, form.focus == 6, false),
                    form_line(
                        "DUCX auth",
                        if form.baidu_auth_bridge == 0 {
                            "Disabled"
                        } else {
                            "DUCX loopback"
                        },
                        form.focus == 7,
                        false,
                    ),
                    form_line(
                        "Code report",
                        bool_name(form.baidu_code_report),
                        form.focus == 8,
                        false,
                    ),
                ]);
            } else if form.preset == "opencode-go" {
                lines.extend([
                    form_line(
                        "Workspace ID",
                        &form.quota_workspace_id,
                        form.focus == 10,
                        false,
                    ),
                    form_line(
                        "New auth cookie",
                        &form.quota_auth_cookie,
                        form.focus == 11,
                        true,
                    ),
                    form_line(
                        "Clear quota auth",
                        bool_name(form.clear_quota),
                        form.focus == 12,
                        false,
                    ),
                ]);
            }
            lines.extend([
                form_line(
                    "Aux upstream",
                    bool_name(form.auxiliary_model_upstream),
                    form.focus == 9,
                    false,
                ),
                Line::from(""),
                Line::from("Empty secrets preserve them unless Clear is enabled."),
                Line::from("[ SAVE ]  [ CANCEL ]   Tab/Up/Down field  Left/Right/Space choose"),
            ]);
            (" \u{25c6} EDIT PROVIDER ", lines)
        }
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

pub(super) fn dialog_popup(area: Rect) -> Rect {
    centered_rect(
        area,
        area.width.min(84),
        area.height.saturating_sub(2).min(30),
    )
}

pub(super) fn render_diagnostics(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
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

pub(super) fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
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
    let keys = if app.busy.is_some() {
        "Esc / Ctrl-C cancel   click this footer to cancel"
    } else {
        match app.page {
            Page::Dashboard => {
                "Left/Right range  Up/Down usage  c quota  s start/stop  r refresh  x doctor"
            }
            Page::Setup => "Up/Down field  Left/Right option  Enter setup  Tab workspace",
            Page::Providers => "a add  u edit  D delete  e enable  t test  m discover  K/J reorder",
            Page::Models => {
                "[ ] provider  Up/Down row  Space/click toggle  a all  n none  s save  d discover"
            }
            Page::Benchmark => {
                "[ ] provider  b/click run  -/+ timeout  ,/. output tokens  r refresh"
            }
            Page::Fusion => "Up/Down model  Space Panel  j Judge  f Final  s save  D disable",
            Page::Integrations => {
                "1-3 Codex  4-5 Claude  6-7 DSH  8-9 OpenCode  p/P Pi  click or press a key"
            }
            Page::System => "s/R gateway  u update  d doctor  F repair  f catalog  l logs",
            Page::Diagnostics => "x doctor  PgUp/PgDn scroll  r refresh  ? help  q quit",
        }
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

pub(super) fn render_busy(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let popup = operation_viewport(area);
    let visible_lines = usize::from(popup.height.saturating_sub(7));
    let output = app
        .diagnostics
        .lines()
        .rev()
        .take(visible_lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|line| {
            Line::from(Span::styled(
                line.to_owned(),
                Style::default().fg(Color::Gray),
            ))
        })
        .collect::<Vec<_>>();
    let mut lines = vec![
        Line::from(Span::styled(
            app.busy.unwrap_or("Working"),
            Style::default().fg(Color::Cyan).bold(),
        )),
        Line::from(""),
    ];
    lines.extend(output);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Esc / Ctrl-C cancel  \u{b7}  progress continues live",
        Style::default().fg(Color::Yellow),
    )));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" \u{25c9} OPERATION ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

pub(super) fn operation_viewport(area: Rect) -> Rect {
    area
}

pub(super) fn render_help(frame: &mut ratatui::Frame<'_>, area: Rect) {
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

pub(super) fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

pub(super) fn form_line(label: &str, value: &str, focused: bool, secret: bool) -> Line<'static> {
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
        Span::styled(
            format!("{} {label:<13}", if focused { "\u{203a}" } else { " " }),
            Style::default().fg(if focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }),
        ),
        Span::styled(format!(" {shown} "), style),
    ])
}

pub(super) fn bool_name(value: bool) -> &'static str {
    if value { "Yes" } else { "No" }
}

pub(super) fn action_label(index: usize, label: &'static str, selected: usize) -> Span<'static> {
    if index == selected {
        Span::styled(
            format!(" {label} "),
            Style::default().fg(Color::Black).bg(Color::Cyan).bold(),
        )
    } else {
        Span::styled(label, Style::default().fg(Color::Gray))
    }
}

pub(super) fn model_id(model: &Value) -> &str {
    model
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| model.as_str())
        .unwrap_or("-")
}

pub(super) fn fusion_models(models: &[Value]) -> Vec<&Value> {
    models
        .iter()
        .filter(|model| !value_str(model, "id", "").starts_with("mixin/fusion/"))
        .collect()
}

pub(super) fn benchmark_status_style(status: &str) -> Style {
    match status {
        "completed" => Style::default().fg(Color::Green).bold(),
        "running" => Style::default().fg(Color::Cyan).bold(),
        "failed" | "interrupted" => Style::default().fg(Color::Red).bold(),
        _ => Style::default().fg(Color::DarkGray),
    }
}

pub(super) fn count_provider_status(providers: &[Value], status: &str) -> usize {
    providers
        .iter()
        .filter(|provider| provider.get("readiness").and_then(Value::as_str) == Some(status))
        .count()
}

pub(super) fn value_str<'a>(value: &'a Value, key: &str, default: &'a str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or(default)
}

pub(super) fn pretty_json_or_text(output: &str) -> String {
    serde_json::from_str::<Value>(output)
        .and_then(|value| serde_json::to_string_pretty(&value))
        .unwrap_or_else(|_| output.trim().to_owned())
}
