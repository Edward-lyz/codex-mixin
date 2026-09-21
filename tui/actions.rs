use super::*;

use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::Rect;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

pub(super) async fn refresh(terminal: &mut TerminalSession, app: &mut App) -> bool {
    app.busy = Some("Refreshing status");
    let _ = terminal.draw(app);
    let refreshed = match Snapshot::load(app.usage_range).await {
        Ok(snapshot) => {
            app.snapshot = snapshot;
            app.clamp_provider_index();
            app.usage_offset = app
                .usage_offset
                .min(app.snapshot.usage.len().saturating_sub(1));
            set_notice(app, false, "Status refreshed.");
            true
        }
        Err(error) => {
            set_notice(app, true, &format!("Refresh failed: {error:#}"));
            false
        }
    };
    app.busy = None;
    refreshed
}

pub(super) async fn refresh_benchmark(app: &mut App) {
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

pub(super) async fn refresh_quota(terminal: &mut TerminalSession, app: &mut App) {
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

pub(super) async fn run_action(
    terminal: &mut TerminalSession,
    app: &mut App,
    label: &'static str,
    args: &[&str],
    refresh_after: bool,
) -> Option<String> {
    app.busy = Some(label);
    app.diagnostics.clear();
    app.diagnostics_scroll = 0;
    let _ = terminal.draw(app);
    let (output, final_notice, final_notice_is_error) =
        match run_cli_with_progress(terminal, app, args).await {
            Ok(Some(output)) => {
                app.diagnostics = pretty_json_or_text(&output);
                (Some(output), format!("{label} completed."), false)
            }
            Ok(None) => (None, format!("{label} cancelled."), false),
            Err(error) => (None, format!("{label} failed: {error:#}"), true),
        };
    app.busy = None;
    let refresh_succeeded = !refresh_after || refresh(terminal, app).await;
    if refresh_succeeded {
        set_notice(app, final_notice_is_error, &final_notice);
    }
    output
}

pub(super) async fn run_cli_with_progress(
    terminal: &mut TerminalSession,
    app: &mut App,
    args: &[&str],
) -> anyhow::Result<Option<String>> {
    let executable = std::env::current_exe().context("resolve current executable")?;
    let mut child = Command::new(&executable)
        .arg("--no-tui")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("run {} --no-tui {}", executable.display(), args.join(" ")))?;
    let stdout = child.stdout.take().context("capture CLI stdout")?;
    let stderr = child.stderr.take().context("capture CLI stderr")?;
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut stdout_output = String::new();
    let mut stderr_output = String::new();

    while stdout_open || stderr_open {
        tokio::select! {
            line = stdout_lines.next_line(), if stdout_open => match line? {
                Some(line) => {
                    stdout_output.push_str(&line);
                    stdout_output.push('\n');
                    show_operation_progress(terminal, app, &line)?;
                }
                None => stdout_open = false,
            },
            line = stderr_lines.next_line(), if stderr_open => match line? {
                Some(line) => {
                    stderr_output.push_str(&line);
                    stderr_output.push('\n');
                    show_operation_progress(terminal, app, &line)?;
                }
                None => stderr_open = false,
            },
            event = read_event() => {
                if let Some(event) = event?
                    && operation_cancel_requested(&event, app.viewport.get())
                {
                    child.start_kill().context("cancel CLI operation")?;
                    child.wait().await.context("wait for cancelled CLI operation")?;
                    return Ok(None);
                }
            }
        }
    }

    let status = child.wait().await.context("wait for CLI operation")?;
    if !status.success() {
        anyhow::bail!(
            "`{}` exited with {}: {}",
            args.join(" "),
            status,
            stderr_output.trim()
        )
    }
    Ok(Some(stdout_output))
}

pub(super) fn show_operation_progress(
    terminal: &mut TerminalSession,
    app: &mut App,
    line: &str,
) -> anyhow::Result<()> {
    let message = line.strip_prefix("MIXIN_PROGRESS ").unwrap_or(line);
    if !message.trim().is_empty() {
        app.notice = message.trim().to_owned();
        if !app.diagnostics.is_empty() {
            app.diagnostics.push('\n');
        }
        app.diagnostics.push_str(message);
        if app.diagnostics.len() > 32 * 1024 {
            while app.diagnostics.len() > 24 * 1024 {
                let Some(line_end) = app.diagnostics.find('\n') else {
                    app.diagnostics.clear();
                    break;
                };
                app.diagnostics.drain(..=line_end);
            }
        }
        terminal.draw(app)?;
    }
    Ok(())
}

pub(super) fn operation_cancel_requested(event: &Event, viewport: Rect) -> bool {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            key.code == KeyCode::Esc
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        }
        Event::Mouse(mouse) => {
            mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && mouse.row >= viewport.y + viewport.height.saturating_sub(3)
        }
        _ => false,
    }
}

pub(super) async fn run_owned_action(
    terminal: &mut TerminalSession,
    app: &mut App,
    label: &'static str,
    args: Vec<String>,
    refresh_after: bool,
) -> Option<String> {
    let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_action(terminal, app, label, &borrowed, refresh_after).await
}

pub(super) async fn apply_provider_changes(terminal: &mut TerminalSession, app: &mut App) {
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

pub(super) fn selected_configured_provider_id(app: &App) -> Option<String> {
    let provider = app.selected_provider()?;
    if provider.get("kind").and_then(Value::as_str) == Some("official") {
        return None;
    }
    provider
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub(super) fn set_notice(app: &mut App, is_error: bool, message: &str) {
    app.notice = message.trim().to_owned();
    app.notice_is_error = is_error;
}

pub(super) async fn run_json(args: &[&str]) -> anyhow::Result<Value> {
    let output = run_cli(args).await?;
    serde_json::from_str(&output).with_context(|| format!("parse `{}` output", args.join(" ")))
}

pub(super) async fn run_cli(args: &[&str]) -> anyhow::Result<String> {
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

pub(super) async fn read_event() -> anyhow::Result<Option<Event>> {
    tokio::task::spawn_blocking(|| -> anyhow::Result<Option<Event>> {
        if event::poll(Duration::from_millis(100)).context("poll terminal event")? {
            return event::read().map(Some).context("read terminal event");
        }
        Ok(None)
    })
    .await
    .context("join terminal event reader")?
}
