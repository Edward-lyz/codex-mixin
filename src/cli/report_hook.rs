//! Mixin-level Baidu code-usage reporting.
//!
//! When a Baidu OneAPI provider opts into reporting, Mixin installs a hook block
//! into the user's real Codex `~/.codex/hooks.json`. Each event invokes
//! `codex-mixin report-hook --event <event>`, which forwards the hook payload to
//! the managed DUCX `data-report` binary with the login-derived username. This
//! reuses DUCX's own reporting mechanism without touching the user's `~/.baidu-cx`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, ensure};
use serde_json::Value;

use codex_mixin::config::load_stored_config;
use codex_mixin::provider::catalog_model_slug;

const MANAGED_HOOK_MARKER: &str = " report-hook --event ";
/// (Codex hooks.json event name, our `--event` argument value).
const REPORT_EVENTS: [(&str, &str); 5] = [
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "user-prompt-submit"),
    ("PreToolUse", "pre-tool-use"),
    ("PostToolUse", "post-tool-use"),
    ("Stop", "stop"),
];

pub(super) fn run(event: &str) -> anyhow::Result<()> {
    let mut hook_body = Vec::new();
    std::io::stdin()
        .read_to_end(&mut hook_body)
        .context("read Codex report hook input")?;
    // Scope reporting to turns that actually use a reporting-enabled Baidu model.
    // Codex hooks fire for every session, so without this filter a non-Baidu
    // session would be reported too.
    let model = hook_body_model(&hook_body);
    let Some(managed) = model
        .as_deref()
        .map(enabled_report_executable)
        .transpose()?
        .flatten()
    else {
        // Reporting disabled, unconfigured, or this turn is not a Baidu model:
        // no-op so the hook never blocks Codex and never over-reports.
        return Ok(());
    };
    let event_argument = match event {
        "session-start" => "--session-start",
        "user-prompt-submit" => "--user-prompt-submit",
        "pre-tool-use" => "--pre-tool-use",
        "post-tool-use" => "--post-tool-use",
        "stop" => "--stop",
        other => anyhow::bail!("unsupported Codex report hook event: {other}"),
    };
    let username = managed_username(&managed.home)?;
    let cwd = hook_body_cwd(&hook_body);
    let (repo, branch) = cwd
        .as_deref()
        .map(git_repo_and_branch)
        .unwrap_or((None, None));
    let mut command = Command::new(&managed.data_report);
    command
        .arg(event_argument)
        .env("HOME", &managed.home)
        .env("DUCX_USERNAME", username)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(repo) = repo {
        command.env("DUCX_REPO", repo);
    }
    if let Some(branch) = branch {
        command.env("DUCX_BRANCH", branch);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("start DUCX data-report {}", managed.data_report.display()))?;
    child
        .stdin
        .take()
        .context("capture DUCX data-report stdin")?
        .write_all(&hook_body)
        .context("write DUCX data-report input")?;
    let status = child.wait().context("wait for DUCX data-report")?;
    ensure!(status.success(), "DUCX data-report exited with {status}");
    Ok(())
}

struct ManagedReport {
    data_report: PathBuf,
    home: PathBuf,
}

/// Resolve the managed DUCX `data-report` binary for the enabled Baidu provider
/// that owns `model` and opted into reporting, or `None` when reporting is off
/// or the model does not belong to such a provider. This keeps reporting scoped
/// to actual Baidu usage even though Codex fires hooks for every session.
fn enabled_report_executable(model: &str) -> anyhow::Result<Option<ManagedReport>> {
    let Some(config) = load_stored_config()? else {
        return Ok(None);
    };
    let Some(provider) = config.providers.iter().find(|provider| {
        provider.enabled
            && provider.request_policy.baidu_code_report
            && provider.model_source == codex_mixin::provider::ProviderModelSource::BaiduOneApi
            && provider_owns_model(provider, model)
    }) else {
        return Ok(None);
    };
    let executable =
        provider.request_policy.ducc_executable.clone().context(
            "Baidu code reporting is enabled but no managed DUCX executable is configured",
        )?;
    managed_report_from_executable(&executable).map(Some)
}

/// True when `model` (as Codex sees it) maps to one of the provider's models,
/// matching the provider-qualified catalog slug for a selected model.
fn provider_owns_model(provider: &codex_mixin::provider::ProviderDefinition, model: &str) -> bool {
    provider
        .selected_models
        .iter()
        .any(|candidate| catalog_model_slug(candidate, &provider.id) == model)
}

/// Given `<home>/.baidu-cx/baidu-cx/bin/ducx`, resolve the sibling data-report
/// binary and the isolated HOME that carries the login state.
fn managed_report_from_executable(executable: &Path) -> anyhow::Result<ManagedReport> {
    let install = executable
        .parent()
        .and_then(Path::parent)
        .context("managed DUCX executable has no install directory")?;
    let data_report = install.join("hooks/data-report");
    ensure!(
        data_report.is_file(),
        "managed DUCX data-report is missing: {}",
        data_report.display()
    );
    let home = install
        .parent()
        .and_then(Path::parent)
        .context("managed DUCX executable has no isolated HOME")?
        .to_owned();
    Ok(ManagedReport { data_report, home })
}

fn managed_username(home: &Path) -> anyhow::Result<String> {
    let login_dir = home.join(".comate/login-user");
    let mut usernames = fs::read_dir(&login_dir)
        .with_context(|| format!("read DUCX login directory {}", login_dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok());
    let username = usernames
        .next()
        .context("DUCX login directory contains no signed-in user")?;
    ensure!(
        usernames.next().is_none(),
        "DUCX login directory contains multiple users; cannot disambiguate reporting identity"
    );
    Ok(username)
}

fn hook_body_cwd(hook_body: &[u8]) -> Option<PathBuf> {
    serde_json::from_slice::<Value>(hook_body)
        .ok()?
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

fn hook_body_model(hook_body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(hook_body)
        .ok()?
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn git_repo_and_branch(cwd: &Path) -> (Option<String>, Option<String>) {
    let repo = git_output(cwd, &["config", "--get", "remote.origin.url"])
        .map(|remote| parse_remote_repo(&remote));
    let branch = git_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]);
    (repo, branch)
}

fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn parse_remote_repo(remote: &str) -> String {
    let trimmed = remote.trim().trim_end_matches(".git");
    let path = if let Some((_, rest)) = trimmed.split_once("://") {
        rest.split_once('/').map(|(_, p)| p).unwrap_or(rest)
    } else if let Some((_, rest)) = trimmed.split_once(':') {
        rest
    } else {
        trimmed
    };
    path.trim_matches('/').to_owned()
}

/// Install or remove the managed reporting hook block in `~/.codex/hooks.json`
/// to match whether any enabled Baidu provider opted into reporting.
pub(super) fn sync_installation() -> anyhow::Result<()> {
    let enabled = load_stored_config()?
        .map(|config| {
            config
                .providers
                .iter()
                .any(|provider| provider.enabled && provider.request_policy.baidu_code_report)
        })
        .unwrap_or(false);
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let hooks_path = PathBuf::from(home).join(".codex/hooks.json");
    if !enabled && !hooks_path.exists() {
        return Ok(());
    }
    let mut document = if hooks_path.exists() {
        serde_json::from_slice::<Value>(&fs::read(&hooks_path)?)
            .with_context(|| format!("parse Codex hooks configuration {}", hooks_path.display()))?
    } else {
        serde_json::json!({ "hooks": {} })
    };
    let hooks = document
        .as_object_mut()
        .context("Codex hooks configuration root is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("Codex hooks field is not an object")?;

    // Always strip our previously-installed managed commands first (idempotent).
    for (event_name, _) in REPORT_EVENTS {
        if let Some(groups) = hooks.get_mut(event_name).and_then(Value::as_array_mut) {
            for group in groups.iter_mut() {
                if let Some(commands) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                    commands.retain(|command| {
                        !command
                            .get("command")
                            .and_then(Value::as_str)
                            .is_some_and(|value| value.contains(MANAGED_HOOK_MARKER))
                    });
                }
            }
            groups.retain(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|commands| !commands.is_empty())
            });
        }
        if hooks
            .get(event_name)
            .and_then(Value::as_array)
            .is_some_and(|g| g.is_empty())
        {
            hooks.remove(event_name);
        }
    }

    if enabled {
        let executable = if cfg!(target_os = "macos") {
            let default_app =
                PathBuf::from("/Applications/Codex Mixin.app/Contents/Resources/codex-mixin");
            if default_app.is_file() {
                default_app
            } else {
                std::env::current_exe().context("resolve codex-mixin executable")?
            }
        } else {
            std::env::current_exe().context("resolve codex-mixin executable")?
        };
        let executable = shell_quote(&executable.to_string_lossy());
        for (event_name, event_argument) in REPORT_EVENTS {
            let group = serde_json::json!({
                "hooks": [{
                    "type": "command",
                    "command": format!("{executable} report-hook --event {event_argument}"),
                    "timeout": 30,
                    "statusMessage": "Reporting Baidu AI code usage"
                }]
            });
            hooks
                .entry(event_name)
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .context("Codex hook event is not an array")?
                .push(group);
        }
    }

    if let Some(parent) = hooks_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut encoded = serde_json::to_vec_pretty(&document)?;
    encoded.push(b'\n');
    fs::write(&hooks_path, &encoded)
        .with_context(|| format!("write Codex hooks configuration {}", hooks_path.display()))?;
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_remotes_into_repo_paths() {
        assert_eq!(
            parse_remote_repo("git@github.com:Edward-lyz/codex-mixin.git"),
            "Edward-lyz/codex-mixin"
        );
        assert_eq!(
            parse_remote_repo("https://github.com/Edward-lyz/codex-mixin.git"),
            "Edward-lyz/codex-mixin"
        );
        assert_eq!(
            parse_remote_repo("ssh://git@icode.baidu.com/user/Work"),
            "user/Work"
        );
    }

    #[test]
    fn reads_cwd_from_hook_body() {
        let body = br#"{"cwd":"/tmp/project","event":"stop"}"#;
        assert_eq!(hook_body_cwd(body), Some(PathBuf::from("/tmp/project")));
        assert_eq!(hook_body_cwd(b"not json"), None);
    }

    #[test]
    fn reads_model_from_hook_body() {
        let body = br#"{"model":"gpt-5.6-luna","cwd":"/tmp"}"#;
        assert_eq!(hook_body_model(body), Some("gpt-5.6-luna".to_owned()));
        assert_eq!(hook_body_model(br#"{"cwd":"/tmp"}"#), None);
    }

    #[test]
    fn scopes_reporting_to_provider_models() {
        let mut provider = codex_mixin::provider::baidu_oneapi_provider("baidu-oneapi", "key");
        provider.selected_models = vec!["GLM-5.2".to_owned()];
        provider.cached_models = vec![codex_mixin::provider::ProviderModel {
            id: "Opus 5".to_owned(),
            ..Default::default()
        }];
        assert!(!provider_owns_model(&provider, "GLM-5.2"));
        assert!(!provider_owns_model(&provider, "Opus 5"));
        assert!(provider_owns_model(
            &provider,
            &catalog_model_slug("GLM-5.2", "baidu-oneapi")
        ));
        assert!(!provider_owns_model(
            &provider,
            &catalog_model_slug("Opus 5", "baidu-oneapi")
        ));
        assert!(!provider_owns_model(&provider, "gpt-4o"));
    }
}
