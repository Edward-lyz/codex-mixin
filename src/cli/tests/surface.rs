use clap::{CommandFactory, Parser};

use crate::cli::tui::StartPage;
use crate::cli::{Cli, requested_tui_start};

#[test]
fn user_facing_command_groups_parse() {
    assert!(Cli::try_parse_from(["codex-mixin", "setup"]).is_ok());
    assert!(
        Cli::try_parse_from([
            "codex-mixin",
            "setup",
            "--preset",
            "baidu-oneapi",
            "--key",
            "test-key",
            "--quota-username",
            "test-user",
            "--codex-mode",
            "skip",
            "--no-start"
        ])
        .is_ok()
    );
    assert!(Cli::try_parse_from(["codex-mixin", "provider", "list"]).is_ok());
    assert!(Cli::try_parse_from(["codex-mixin", "service", "start", "--foreground"]).is_ok());
    assert!(Cli::try_parse_from(["codex-mixin", "connect", "codex", "--custom-only"]).is_ok());
    assert!(Cli::try_parse_from(["codex-mixin", "connect", "claude"]).is_ok());
    assert!(Cli::try_parse_from(["codex-mixin", "connect", "dsh"]).is_ok());
    assert!(
        Cli::try_parse_from([
            "codex-mixin",
            "connect",
            "remove",
            "dsh",
            "--dsh-home",
            "/tmp/dsh-home",
        ])
        .is_ok()
    );
    assert!(Cli::try_parse_from(["codex-mixin", "info", "--json"]).is_ok());
}

#[test]
fn no_tui_flag_preserves_plain_default_command() {
    let interactive = Cli::try_parse_from(["codex-mixin"]).unwrap();
    assert!(!interactive.no_tui);
    assert!(interactive.command.is_none());

    let plain = Cli::try_parse_from(["codex-mixin", "--no-tui"]).unwrap();
    assert!(plain.no_tui);
    assert!(plain.command.is_none());

    let explicit = Cli::try_parse_from(["codex-mixin", "info"]).unwrap();
    assert!(!explicit.no_tui);
    assert!(explicit.command.is_some());
}

#[test]
fn bare_setup_opens_the_same_tui_control_center() {
    let setup = Cli::try_parse_from(["codex-mixin", "setup"]).unwrap();
    assert_eq!(requested_tui_start(&setup, true), Some(StartPage::Setup));
    assert_eq!(requested_tui_start(&setup, false), None);

    let plain = Cli::try_parse_from(["codex-mixin", "--no-tui", "setup"]).unwrap();
    assert_eq!(requested_tui_start(&plain, true), None);
}

#[test]
fn setup_help_lists_provider_presets() {
    let mut help = Vec::new();
    Cli::command()
        .find_subcommand_mut("setup")
        .expect("setup command")
        .write_long_help(&mut help)
        .unwrap();
    let help = String::from_utf8(help).unwrap();
    for preset in [
        "custom",
        "baidu-oneapi",
        "openrouter",
        "deepseek",
        "opencode-go",
        "aws-bedrock",
    ] {
        assert!(
            help.contains(preset),
            "missing preset {preset} in setup help:\n{help}"
        );
    }
}

#[test]
fn setup_rejects_unknown_preset_values() {
    let error = Cli::try_parse_from(["codex-mixin", "setup", "--preset", "not-a-preset"])
        .expect_err("unknown preset must fail parse");
    let message = error.to_string();
    assert!(
        message.contains("baidu-oneapi") || message.contains("possible values"),
        "unexpected error: {message}"
    );
}

#[test]
fn top_level_help_only_lists_user_facing_commands() {
    let mut help = Vec::new();
    Cli::command().write_long_help(&mut help).unwrap();
    let help = String::from_utf8(help).unwrap();

    for command in ["setup", "provider", "service", "connect", "info", "doctor"] {
        assert!(help.contains(command), "missing {command} in help:\n{help}");
    }
    for legacy_command in ["install-codex", "serve", "migrate-history", "benchmark"] {
        assert!(
            !help.contains(legacy_command),
            "legacy command {legacy_command} leaked into help:\n{help}"
        );
    }
}

#[test]
fn provider_select_accepts_an_empty_allowlist() {
    assert!(Cli::try_parse_from(["codex-mixin", "providers", "select", "provider-a"]).is_ok());
}

#[test]
fn macos_bridge_commands_accept_multi_provider_arguments() {
    assert!(
        Cli::try_parse_from([
            "codex-mixin",
            "benchmark",
            "start",
            "--timeout-seconds",
            "10",
            "--provider",
            "provider-a",
            "--provider",
            "provider-b",
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "codex-mixin",
            "fusion",
            "set",
            "--profile-json",
            r#"{"id":"default","panel_models":["model-provider-a"],"judge_model":"model-provider-a","final_model":"model-provider-a"}"#,
            "--replace-id",
            "default",
        ])
        .is_ok()
    );
    assert!(Cli::try_parse_from(["codex-mixin", "fusion", "delete", "--id", "default"]).is_ok());
}
