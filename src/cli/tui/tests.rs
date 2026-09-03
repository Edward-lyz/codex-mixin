use super::*;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};

#[test]
fn snapshot_handles_unconfigured_status() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": false}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: Vec::new(),
        fusion_profile: None,
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
        preset_index: 5,
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
fn baidu_provider_form_submits_gui_parity_settings() {
    let form = AddProviderForm {
        api_key: "secret".to_owned(),
        quota_username: "owner".to_owned(),
        image_generation_path: "/v1/images/generations".to_owned(),
        baidu_auth_bridge: 1,
        baidu_code_report: true,
        auxiliary_model_upstream: true,
        ..AddProviderForm::default()
    };
    let args = form.args().unwrap();
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--baidu-auth-bridge", "ducx_loopback"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--baidu-code-report", "true"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--auxiliary-model-upstream", "true"])
    );
    assert!(
        args.windows(2)
            .any(|pair| { pair == ["--image-generation-path", "/v1/images/generations"] })
    );
}

#[test]
fn aws_bedrock_form_submits_aksk_credentials() {
    let form = AddProviderForm {
        preset_index: 4,
        aws_access_key_id: "AKIDEXAMPLE".to_owned(),
        aws_secret_access_key: "secret".to_owned(),
        aws_session_token: "session".to_owned(),
        aws_region: "eu-west-1".to_owned(),
        ..AddProviderForm::default()
    };

    let args = form.args().unwrap();

    assert!(
        args.windows(2)
            .any(|pair| pair == ["--preset", "aws-bedrock"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--aws-access-key-id", "AKIDEXAMPLE"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--aws-secret-access-key", "secret"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--aws-session-token", "session"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--aws-region", "eu-west-1"])
    );
    assert!(!args.iter().any(|argument| argument == "--key"));
    assert!(!args.iter().any(|argument| argument == "--base-url"));
}

#[test]
fn edit_aws_bedrock_form_preserves_and_clears_credentials_explicitly() {
    let provider = serde_json::json!({
        "id": "aws-bedrock",
        "kind": "configured",
        "preset_id": "aws-bedrock",
        "enabled": false,
        "display_name": "Amazon Bedrock",
        "base_url": "https://bedrock-mantle.eu-west-1.api.aws/anthropic",
        "aws_sigv4_configured": true,
        "aws_region": "eu-west-1",
        "aws_session_token_configured": true,
        "auxiliary_model_upstream": false
    });
    let mut form = EditProviderForm::from_provider(&provider).unwrap();

    let preserved = form.args().unwrap();
    assert!(
        preserved
            .windows(2)
            .any(|pair| pair == ["--aws-region", "eu-west-1"])
    );
    assert!(!preserved.iter().any(|argument| {
        matches!(
            argument.as_str(),
            "--aws-access-key-id"
                | "--aws-secret-access-key"
                | "--aws-session-token"
                | "--clear-aws-session-token"
                | "--clear-aws-credentials"
        )
    }));

    form.clear_aws_session_token = true;
    assert!(
        form.args()
            .unwrap()
            .iter()
            .any(|argument| argument == "--clear-aws-session-token")
    );

    form.clear_aws_session_token = false;
    form.clear_aws_credentials = true;
    assert!(
        form.args()
            .unwrap()
            .iter()
            .any(|argument| argument == "--clear-aws-credentials")
    );
}

#[test]
fn edit_provider_form_preserves_and_clears_credentials_explicitly() {
    let provider = serde_json::json!({
        "id": "baidu-oneapi",
        "kind": "configured",
        "preset_id": "baidu-oneapi",
        "enabled": false,
        "display_name": "Baidu OneAPI",
        "base_url": "https://example.test",
        "api_key_configured": true,
        "quota_username": "owner",
        "baidu_auth_bridge": "disabled",
        "baidu_code_report": false,
        "auxiliary_model_upstream": false
    });
    let mut form = EditProviderForm::from_provider(&provider).unwrap();
    let preserved = form.args().unwrap();
    assert!(!preserved.iter().any(|argument| argument == "--key"));
    assert!(!preserved.iter().any(|argument| argument == "--clear-key"));

    form.clear_key = true;
    form.baidu_auth_bridge = 1;
    form.baidu_code_report = true;
    form.auxiliary_model_upstream = true;
    let changed = form.args().unwrap();
    assert!(changed.iter().any(|argument| argument == "--clear-key"));
    assert!(
        changed
            .windows(2)
            .any(|pair| pair == ["--baidu-auth-bridge", "ducx_loopback"])
    );
    assert!(
        changed
            .windows(2)
            .any(|pair| pair == ["--baidu-code-report", "true"])
    );
    assert!(
        changed
            .windows(2)
            .any(|pair| pair == ["--auxiliary-model-upstream", "true"])
    );
}

#[test]
fn provider_dialog_accepts_mouse_field_and_action_clicks() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": true, "gateway": "stopped"}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: Vec::new(),
        fusion_profile: None,
        refreshed_at: Instant::now(),
    };
    let mut app = App::new(snapshot, StartPage::Dashboard);
    app.viewport.set(Rect::new(0, 0, 100, 30));
    app.dialog = Some(Dialog::AddProvider(AddProviderForm::default()));
    let popup = provider_editor_form_area(app.viewport.get());

    assert_eq!(
        handle_mouse_event(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            popup.x + 20,
            popup.y + 5,
        ),
        Action::None
    );
    let Some(Dialog::AddProvider(form)) = app.dialog.as_ref() else {
        panic!("add provider dialog should remain open")
    };
    assert_eq!(form.focus, 9);
    assert_eq!(form.baidu_auth_bridge, 1);

    assert_eq!(
        handle_mouse_event(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            popup.x + 2,
            popup.y + 10,
        ),
        Action::SubmitDialog
    );
}

#[test]
fn provider_editor_replaces_the_primary_workspace() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": true, "gateway": "stopped"}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: Vec::new(),
        fusion_profile: None,
        refreshed_at: Instant::now(),
    };
    let mut app = App::new(snapshot, StartPage::Dashboard);
    app.dialog = Some(Dialog::AddProvider(AddProviderForm::default()));
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();

    let buffer = terminal.backend().buffer();
    let mut rendered = String::new();
    for row in 0..buffer.area.height {
        for column in 0..buffer.area.width {
            if let Some(cell) = buffer.cell((column, row)) {
                rendered.push_str(cell.symbol());
            }
        }
        rendered.push('\n');
    }
    assert!(rendered.contains("\u{2190} Providers / Add provider"));
    assert!(rendered.contains("PROVIDER WORKSPACE"));
    assert!(!rendered.contains("\u{21af} Speed"));
}

#[test]
fn fusion_workspace_builds_the_complete_profile() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": true, "gateway": "stopped"}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: vec![
            serde_json::json!({"id": "model-a"}),
            serde_json::json!({"id": "model-b"}),
            serde_json::json!({"id": "mixin/fusion/old"}),
        ],
        fusion_profile: None,
        refreshed_at: Instant::now(),
    };
    let mut form = FusionForm::new(&snapshot);
    form.profile_id = "review".to_owned();
    form.min_successful = 2;
    form.timeout_ms = 180_000;
    form.show_intermediate_results = false;
    let args = form.args(&snapshot.models).unwrap();
    let profile_index = args
        .iter()
        .position(|argument| argument == "--profile-json")
        .unwrap();
    let profile: Value = serde_json::from_str(&args[profile_index + 1]).unwrap();
    assert_eq!(profile["id"], "review");
    assert_eq!(
        profile["panel_models"],
        serde_json::json!(["model-a", "model-b"])
    );
    assert_eq!(profile["judge_model"], "model-a");
    assert_eq!(profile["final_model"], "model-b");
    assert_eq!(profile["min_successful"], 2);
    assert_eq!(profile["timeout_ms"], 180_000);
    assert_eq!(profile["show_intermediate_results"], false);
}

#[test]
fn fusion_workspace_supports_mouse_model_selection() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": true, "gateway": "stopped"}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: vec![serde_json::json!({"id": "model-a"})],
        fusion_profile: None,
        refreshed_at: Instant::now(),
    };
    let mut app = App::new(snapshot, StartPage::Dashboard);
    app.page = Page::Fusion;
    app.viewport.set(Rect::new(0, 0, 100, 30));
    app.fusion.panel_models.clear();

    assert_eq!(
        handle_mouse_event(&mut app, MouseEventKind::Down(MouseButton::Left), 10, 9,),
        Action::None
    );
    assert!(app.fusion.panel_models.contains("model-a"));
}

#[test]
fn dashboard_usage_range_supports_keyboard_and_mouse() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": true, "gateway": "stopped"}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: Vec::new(),
        fusion_profile: None,
        refreshed_at: Instant::now(),
    };
    let mut app = App::new(snapshot, StartPage::Dashboard);
    app.viewport.set(Rect::new(0, 0, 100, 30));
    app.usage_range = 0;

    assert_eq!(handle_page_event(&mut app, KeyCode::Right), Action::Refresh);
    assert_eq!(app.usage_range, 1);
    assert_eq!(
        handle_mouse_event(&mut app, MouseEventKind::Down(MouseButton::Left), 15, 17,),
        Action::Refresh
    );
    assert_eq!(app.usage_range, 0);
}

#[test]
fn every_workspace_renders_at_eighty_by_twenty_four() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": true, "gateway": "stopped"}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: vec![serde_json::json!({"id": "model-a"})],
        fusion_profile: None,
        refreshed_at: Instant::now(),
    };
    let mut app = App::new(snapshot, StartPage::Dashboard);
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    for page in Page::ALL {
        app.page = page;
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }
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
        models: Vec::new(),
        fusion_profile: None,
        refreshed_at: Instant::now(),
    };
    let app = App::new(snapshot, StartPage::Dashboard);
    assert_eq!(app.page, Page::Setup);
}

#[test]
fn tab_moves_to_the_next_workspace_once() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": true, "gateway": "stopped"}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: Vec::new(),
        fusion_profile: None,
        refreshed_at: Instant::now(),
    };
    let mut app = App::new(snapshot, StartPage::Dashboard);

    assert_eq!(
        handle_event(
            &mut app,
            Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Tab,
                KeyModifiers::NONE,
            )),
        ),
        Action::None
    );
    assert_eq!(app.page, Page::Setup);

    assert_eq!(
        handle_event(
            &mut app,
            Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Tab,
                KeyModifiers::NONE,
            )),
        ),
        Action::None
    );
    assert_eq!(app.page, Page::Providers);
}

#[test]
fn mouse_selects_tabs_and_integration_actions() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": true, "gateway": "stopped"}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: Vec::new(),
        fusion_profile: None,
        refreshed_at: Instant::now(),
    };
    let mut app = App::new(snapshot, StartPage::Dashboard);
    app.viewport.set(Rect::new(0, 0, 100, 30));
    assert_eq!(
        handle_mouse_event(&mut app, MouseEventKind::Down(MouseButton::Left), 8, 4),
        Action::None
    );
    assert_eq!(app.page, Page::Setup);
    app.page = Page::Dashboard;
    assert_eq!(
        handle_mouse_event(&mut app, MouseEventKind::Down(MouseButton::Left), 8, 5),
        Action::None
    );
    assert_eq!(app.page, Page::Dashboard);
    app.page = Page::Integrations;
    assert_eq!(
        handle_mouse_event(&mut app, MouseEventKind::Down(MouseButton::Left), 90, 8),
        Action::ConfirmUninstallCodex
    );
    assert_eq!(integration_action(9), Action::ConnectPi);
    assert_eq!(integration_action(10), Action::ConfirmUninstallPi);
}

#[test]
fn running_operation_can_be_cancelled_by_keyboard_or_footer_click() {
    let viewport = Rect::new(0, 0, 100, 30);
    assert!(operation_cancel_requested(
        &Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )),
        viewport,
    ));
    assert!(operation_cancel_requested(
        &Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 50,
            row: 28,
            modifiers: KeyModifiers::NONE,
        }),
        viewport,
    ));
    assert!(!operation_cancel_requested(
        &Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 50,
            row: 20,
            modifiers: KeyModifiers::NONE,
        }),
        viewport,
    ));
}

#[test]
fn operation_uses_the_full_terminal_for_qr_output() {
    let terminal = Rect::new(0, 0, 120, 50);

    assert_eq!(operation_viewport(terminal), terminal);
}

#[test]
fn operation_keeps_the_complete_qr_visible_on_a_large_terminal() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": true, "gateway": "stopped"}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: Vec::new(),
        fusion_profile: None,
        refreshed_at: Instant::now(),
    };
    let mut app = App::new(snapshot, StartPage::Dashboard);
    app.busy = Some("DUCX authentication");
    app.diagnostics = (0..32)
        .map(|row| match row {
            0 => "QR-TOP".to_owned(),
            31 => "QR-BOTTOM".to_owned(),
            _ => format!("QR-{row:02}  \u{2588}\u{2588}  \u{2588}\u{2588}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let backend = ratatui::backend::TestBackend::new(120, 50);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();

    let buffer = terminal.backend().buffer();
    let mut rendered = String::new();
    for row in 0..buffer.area.height {
        for column in 0..buffer.area.width {
            if let Some(cell) = buffer.cell((column, row)) {
                rendered.push_str(cell.symbol());
            }
        }
        rendered.push('\n');
    }
    assert!(rendered.contains("QR-TOP"));
    assert!(rendered.contains("QR-BOTTOM"));
}

#[test]
fn action_labels_only_accept_clicks_on_visible_text() {
    assert_eq!(
        clicked_action_label(31, 31, &PROVIDER_ACTION_LABELS),
        Some(0)
    );
    assert_eq!(clicked_action_label(36, 31, &PROVIDER_ACTION_LABELS), None);
    assert_eq!(
        clicked_action_label(39, 31, &PROVIDER_ACTION_LABELS),
        Some(1)
    );
    assert_eq!(clicked_action_label(7, 1, &MODEL_ACTION_LABELS), None);
    assert_eq!(clicked_action_label(10, 1, &MODEL_ACTION_LABELS), Some(1));
}

#[test]
fn setup_workspace_renders_at_eighty_by_twenty_four() {
    let snapshot = Snapshot {
        status: serde_json::json!({"configured": false}),
        providers: Vec::new(),
        codex_install_mode: None,
        benchmark: None,
        usage: Vec::new(),
        models: Vec::new(),
        fusion_profile: None,
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
