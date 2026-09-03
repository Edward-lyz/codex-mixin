use super::*;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Span;

pub(super) fn handle_event(app: &mut App, event: Event) -> Action {
    let key = match event {
        Event::Mouse(mouse) => return handle_mouse_event(app, mouse.kind, mouse.column, mouse.row),
        Event::Key(key) => key,
        _ => return Action::None,
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
        KeyCode::Tab => {
            app.next_page(1);
            return Action::None;
        }
        KeyCode::BackTab => {
            app.next_page(-1);
            return Action::None;
        }
        _ => {}
    }
    if app.page == Page::Setup {
        return handle_setup_event(app, key.code, key.modifiers);
    }
    if app.page == Page::Fusion {
        return handle_fusion_event(app, key.code);
    }
    match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('?') => {
            app.help_visible = true;
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
pub(super) fn handle_mouse_event(
    app: &mut App,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> Action {
    if app.dialog.is_some() {
        return handle_dialog_mouse_event(app, kind, column, row);
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
            Page::Fusion => {
                app.fusion.model_index = (app.fusion.model_index as isize + offset).clamp(
                    0,
                    fusion_models(&app.snapshot.models).len().saturating_sub(1) as isize,
                ) as usize;
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
    if row == tabs_y {
        let compact_tabs = area.width < 110;
        let mut start = area.x;
        for page in Page::ALL {
            let end = start + Span::raw(page.tab_title(compact_tabs)).width() as u16 + 2;
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
                if matches!(app.setup.focus, 0 | 9 | 10 | 11) {
                    app.setup.provider.toggle_focused(1);
                }
            } else if line == fields.len() {
                app.setup.focus = 17;
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
            } else if row == body.y + body.height.saturating_sub(2) {
                return match clicked_action_label(column, body.x + 31, &PROVIDER_ACTION_LABELS) {
                    Some(0) => Action::AddProvider,
                    Some(1) => Action::EditProvider,
                    Some(2) => Action::RemoveProvider,
                    Some(3) => Action::ToggleProvider,
                    Some(4) => Action::TestProvider,
                    Some(5) => Action::DiscoverModels,
                    _ => Action::None,
                };
            }
            Action::None
        }
        Page::Models => {
            if row == body.y + 2 {
                return match clicked_action_label(column, body.x + 1, &MODEL_ACTION_LABELS) {
                    Some(0) => Action::ApplyModels,
                    Some(1) => {
                        app.model_draft = app
                            .selected_models()
                            .into_iter()
                            .map(|model| model_id(model).to_owned())
                            .collect();
                        Action::None
                    }
                    Some(2) => {
                        app.model_draft.clear();
                        Action::None
                    }
                    Some(3) => Action::DiscoverModels,
                    Some(4) => Action::ProbeModels,
                    _ => Action::None,
                };
            }
            if row > body.y + 5 {
                let index = row.saturating_sub(body.y + 6) as usize;
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
        Page::Fusion => {
            let split = body.x + body.width.saturating_mul(58) / 100;
            if column < split {
                if row > body.y + 1 {
                    let index = row.saturating_sub(body.y + 2) as usize;
                    if index < fusion_models(&app.snapshot.models).len() {
                        app.fusion.model_index = index;
                        return handle_fusion_event(app, KeyCode::Char(' '));
                    }
                }
                return Action::None;
            }
            let content_row = row.saturating_sub(body.y + 1);
            match content_row {
                0 => {
                    app.fusion.editing_profile_id = true;
                    Action::None
                }
                7 => {
                    app.fusion.show_intermediate_results = !app.fusion.show_intermediate_results;
                    Action::None
                }
                8 => {
                    app.fusion.panel_tools_enabled = !app.fusion.panel_tools_enabled;
                    Action::None
                }
                10 if column < split + 12 => Action::SaveFusion,
                10 => Action::ConfirmDisableFusion,
                _ => Action::None,
            }
        }
        Page::Integrations => {
            let relative_row = row.saturating_sub(body.y);
            let card = (usize::from(relative_row) * 5 / usize::from(body.height.max(1))).min(4);
            let relative_column = column.saturating_sub(body.x);
            app.integration_index = match card {
                0 => (usize::from(relative_column) * 3 / usize::from(body.width.max(1))).min(2),
                1 => 3 + (usize::from(relative_column) * 2 / usize::from(body.width.max(1))).min(1),
                2 => 5 + (usize::from(relative_column) * 2 / usize::from(body.width.max(1))).min(1),
                3 => 7 + (usize::from(relative_column) * 2 / usize::from(body.width.max(1))).min(1),
                _ => 9 + (usize::from(relative_column) * 2 / usize::from(body.width.max(1))).min(1),
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
        Page::Dashboard => {
            if row == body.y + 10 {
                let start = body.x + 15;
                if let Some(range) = clicked_action_label(column, start, &USAGE_RANGE_LABELS) {
                    app.usage_range = range;
                    app.usage_offset = 0;
                    return Action::Refresh;
                }
            }
            Action::None
        }
        Page::Diagnostics => Action::None,
    }
}

pub(super) fn handle_dialog_mouse_event(
    app: &mut App,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> Action {
    if kind != MouseEventKind::Down(MouseButton::Left) {
        return Action::None;
    }
    let editor_open = matches!(
        app.dialog.as_ref(),
        Some(Dialog::AddProvider(_) | Dialog::EditProvider(_))
    );
    let popup = if editor_open {
        provider_editor_form_area(app.viewport.get())
    } else {
        dialog_popup(app.viewport.get())
    };
    if column <= popup.x
        || column >= popup.x + popup.width.saturating_sub(1)
        || row <= popup.y
        || row >= popup.y + popup.height.saturating_sub(1)
    {
        return Action::None;
    }
    let content_row = row.saturating_sub(popup.y + 1) as usize;
    let Some(dialog) = app.dialog.as_mut() else {
        return Action::None;
    };
    match dialog {
        Dialog::AddProvider(form) => {
            let fields = form.active_fields();
            if let Some(field) = fields.get(content_row) {
                form.focus = *field;
                if matches!(form.focus, 0 | 9 | 10 | 11) {
                    form.toggle_focused(1);
                }
                return Action::None;
            }
            if content_row == fields.len() + 1 {
                if column < popup.x + 10 {
                    return Action::SubmitDialog;
                }
                if column < popup.x + 22 {
                    app.dialog = None;
                }
            }
            Action::None
        }
        Dialog::EditProvider(form) => {
            let fields = form.active_fields();
            if let Some(field) = content_row
                .checked_sub(2)
                .and_then(|index| fields.get(index))
            {
                form.focus = *field;
                if matches!(form.focus, 5 | 7 | 8 | 9 | 12 | 17 | 18) {
                    form.toggle_focused(1);
                }
                return Action::None;
            }
            if content_row == fields.len() + 4 {
                if column < popup.x + 11 {
                    return Action::SubmitDialog;
                }
                if column < popup.x + 23 {
                    app.dialog = None;
                }
            }
            Action::None
        }
        Dialog::ConfirmRemove(_) => {
            if content_row == 3 {
                if column < popup.x + 16 {
                    return Action::ConfirmRemoveProvider;
                }
                app.dialog = None;
            }
            Action::None
        }
        Dialog::ConfirmOperation(_) => {
            if content_row == 4 {
                if column < popup.x + 18 {
                    return Action::RunConfirmedOperation;
                }
                app.dialog = None;
            }
            Action::None
        }
        Dialog::ConfirmDisableFusion(_) => {
            if content_row == 3 {
                if column < popup.x + 18 {
                    return Action::DisableFusion;
                }
                app.dialog = None;
            }
            Action::None
        }
    }
}

pub(super) fn integration_action(index: usize) -> Action {
    match index {
        0 => Action::ConnectCodexOfficial,
        1 => Action::ConnectCodexCustom,
        2 => Action::ConfirmUninstallCodex,
        3 => Action::ConnectClaude,
        4 => Action::ConfirmUninstallClaude,
        5 => Action::ConnectDsh,
        6 => Action::ConfirmUninstallDsh,
        7 => Action::ConnectOpenCode,
        8 => Action::ConfirmUninstallOpenCode,
        9 => Action::ConnectPi,
        _ => Action::ConfirmUninstallPi,
    }
}

pub(super) fn system_action(index: usize) -> Action {
    match index {
        0 => Action::ToggleGateway,
        1 => Action::ConfirmUpdate,
        2 => Action::ConfirmRepair,
        _ => Action::RefreshCatalog,
    }
}

pub(super) fn clicked_action_label(column: u16, start: u16, labels: &[&str]) -> Option<usize> {
    let mut cursor = start;
    for (index, label) in labels.iter().enumerate() {
        let end = cursor.saturating_add(label.len() as u16);
        if column >= cursor && column < end {
            return Some(index);
        }
        cursor = end.saturating_add(2);
    }
    None
}

pub(super) fn handle_page_event(app: &mut App, code: KeyCode) -> Action {
    match app.page {
        Page::Setup => Action::None,
        Page::Fusion => Action::None,
        Page::Providers => handle_provider_event(app, code),
        Page::Models => handle_model_event(app, code),
        Page::Benchmark => handle_benchmark_event(app, code),
        Page::Integrations => match code {
            KeyCode::Left | KeyCode::Up => {
                app.integration_index = app.integration_index.saturating_sub(1);
                Action::None
            }
            KeyCode::Right | KeyCode::Down => {
                app.integration_index = (app.integration_index + 1).min(10);
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
            KeyCode::Char('8') => Action::ConnectOpenCode,
            KeyCode::Char('9') => Action::ConfirmUninstallOpenCode,
            KeyCode::Char('p') => Action::ConnectPi,
            KeyCode::Char('P') => Action::ConfirmUninstallPi,
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
            KeyCode::Left => {
                app.usage_range = app.usage_range.saturating_sub(1);
                app.usage_offset = 0;
                Action::Refresh
            }
            KeyCode::Right => {
                app.usage_range = (app.usage_range + 1).min(USAGE_RANGE_LABELS.len() - 1);
                app.usage_offset = 0;
                Action::Refresh
            }
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

pub(super) fn handle_setup_event(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Action {
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
        KeyCode::Down => app.setup.move_focus(1),
        KeyCode::Up => app.setup.move_focus(-1),
        KeyCode::Left if app.setup.focus != 17 => app.setup.provider.toggle_focused(-1),
        KeyCode::Right if app.setup.focus != 17 => app.setup.provider.toggle_focused(1),
        KeyCode::Char(' ') if app.setup.focus != 17 => app.setup.provider.toggle_focused(1),
        KeyCode::Left if app.setup.focus == 17 => {
            app.setup.codex_mode = app.setup.codex_mode.saturating_sub(1);
        }
        KeyCode::Right if app.setup.focus == 17 => {
            app.setup.codex_mode = (app.setup.codex_mode + 1).min(2);
        }
        KeyCode::Backspace if app.setup.focus != 17 => {
            if let Some(value) = app.setup.provider.focused_text() {
                value.pop();
            }
        }
        KeyCode::Char(character) if app.setup.focus != 17 => {
            if let Some(value) = app.setup.provider.focused_text() {
                value.push(character);
            }
        }
        _ => {}
    }
    Action::None
}

pub(super) fn handle_provider_event(app: &mut App, code: KeyCode) -> Action {
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
        KeyCode::Char('K') => Action::MoveProviderUp,
        KeyCode::Char('J') => Action::MoveProviderDown,
        _ => Action::None,
    }
}

pub(super) fn handle_model_event(app: &mut App, code: KeyCode) -> Action {
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

pub(super) fn handle_benchmark_event(app: &mut App, code: KeyCode) -> Action {
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

pub(super) fn handle_fusion_event(app: &mut App, code: KeyCode) -> Action {
    if app.fusion.editing_profile_id {
        match code {
            KeyCode::Esc | KeyCode::Enter => app.fusion.editing_profile_id = false,
            KeyCode::Backspace => {
                app.fusion.profile_id.pop();
            }
            KeyCode::Char(character) if character != '/' => {
                app.fusion.profile_id.push(character);
            }
            _ => {}
        }
        return Action::None;
    }
    match code {
        KeyCode::Char('q') => return Action::Quit,
        KeyCode::Char('?') => {
            app.help_visible = true;
        }
        KeyCode::Char('r') => return Action::Refresh,
        KeyCode::Char('x') => return Action::RunDoctor,
        KeyCode::Up => app.fusion.model_index = app.fusion.model_index.saturating_sub(1),
        KeyCode::Down => {
            app.fusion.model_index = (app.fusion.model_index + 1)
                .min(fusion_models(&app.snapshot.models).len().saturating_sub(1));
        }
        KeyCode::Char(' ') => {
            if let Some(model) = app
                .fusion
                .selected_model(&app.snapshot.models)
                .map(str::to_owned)
                && !app.fusion.panel_models.remove(&model)
            {
                if app.fusion.panel_models.len() < 8 {
                    app.fusion.panel_models.insert(model);
                } else {
                    set_notice(app, true, "Fusion supports at most 8 Panel models.");
                }
            }
        }
        KeyCode::Char('j') => {
            if let Some(model) = app.fusion.selected_model(&app.snapshot.models) {
                app.fusion.judge_model = model.to_owned();
            }
        }
        KeyCode::Char('f') => {
            if let Some(model) = app.fusion.selected_model(&app.snapshot.models) {
                app.fusion.final_model = model.to_owned();
            }
        }
        KeyCode::Char('e') => app.fusion.editing_profile_id = true,
        KeyCode::Char('-') => {
            app.fusion.min_successful = app.fusion.min_successful.saturating_sub(1).max(1);
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.fusion.min_successful =
                (app.fusion.min_successful + 1).min(app.fusion.panel_models.len().max(1));
        }
        KeyCode::Left => {
            app.fusion.timeout_ms = app.fusion.timeout_ms.saturating_sub(30_000).max(30_000)
        }
        KeyCode::Right => app.fusion.timeout_ms = app.fusion.timeout_ms.saturating_add(30_000),
        KeyCode::Char('i') => {
            app.fusion.show_intermediate_results = !app.fusion.show_intermediate_results;
        }
        KeyCode::Char('t') => {
            app.fusion.panel_tools_enabled = !app.fusion.panel_tools_enabled;
        }
        KeyCode::Char('s') => return Action::SaveFusion,
        KeyCode::Char('D') => return Action::ConfirmDisableFusion,
        _ => {}
    }
    Action::None
}

pub(super) fn select_provider(app: &mut App, offset: isize) {
    let last = app.snapshot.providers.len().saturating_sub(1) as isize;
    app.provider_index = (app.provider_index as isize + offset).clamp(0, last) as usize;
    app.load_model_draft();
}

pub(super) fn handle_dialog_event(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Action {
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
        Dialog::ConfirmDisableFusion(_) => match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Action::DisableFusion,
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
                KeyCode::Tab | KeyCode::Down => form.move_focus(1),
                KeyCode::BackTab | KeyCode::Up => form.move_focus(-1),
                KeyCode::Left => form.toggle_focused(-1),
                KeyCode::Right | KeyCode::Char(' ') => form.toggle_focused(1),
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
                KeyCode::Tab | KeyCode::Down => form.move_focus(1),
                KeyCode::BackTab | KeyCode::Up => form.move_focus(-1),
                KeyCode::Left => form.toggle_focused(-1),
                KeyCode::Right | KeyCode::Char(' ') => form.toggle_focused(1),
                KeyCode::Backspace => {
                    if let Some(value) = form.focused_text() {
                        value.pop();
                    }
                }
                KeyCode::Char(character) => {
                    let focus = form.focus;
                    if let Some(value) = form.focused_text() {
                        value.push(character);
                        if focus == 4 {
                            form.clear_key = false;
                        } else if focus == 11 {
                            form.clear_quota = false;
                        } else if matches!(focus, 14 | 15) {
                            form.clear_aws_credentials = false;
                        } else if focus == 16 {
                            form.clear_aws_session_token = false;
                            form.clear_aws_credentials = false;
                        }
                    }
                }
                _ => {}
            }
            Action::None
        }
    }
}
