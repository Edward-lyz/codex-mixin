//! Pure rendering and inspection of OS startup-service definitions. These
//! functions compile on every platform so their escaping and matching rules
//! are tested everywhere, independent of which adapter is active.

use std::path::Path;

use anyhow::bail;
use serde::Deserialize;

pub(super) const LAUNCHD_LABEL: &str = "local.codex-mixin.service";
pub(super) const LAUNCHD_THROTTLE_SECONDS: i64 = 10;
pub(super) const SYSTEMD_UNIT_NAME: &str = "codex-mixin.service";
pub(super) const WINDOWS_TASK_NAME: &str = "Codex Mixin Gateway";

/// Command line of the supervised gateway process.
#[derive(Clone, Copy, Debug)]
pub struct StartupServiceSpec<'a> {
    pub executable: &'a Path,
    pub log_file: &'a Path,
}

fn utf8_path<'a>(label: &str, path: &'a Path) -> anyhow::Result<&'a str> {
    let Some(value) = path.to_str() else {
        bail!("{label} path is not valid UTF-8: {}", path.display());
    };
    if value
        .chars()
        .any(|character| matches!(character, '\n' | '\r' | '\0'))
    {
        bail!("{label} path contains a line break or NUL character");
    }
    Ok(value)
}

fn gateway_arguments<'a>(spec: &StartupServiceSpec<'a>) -> anyhow::Result<[&'a str; 4]> {
    Ok([
        utf8_path("gateway executable", spec.executable)?,
        "start",
        "--log-file",
        utf8_path("gateway log", spec.log_file)?,
    ])
}

// ---------------------------------------------------------------- launchd

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn render_launchd_plist(
    spec: &StartupServiceSpec<'_>,
    working_directory: &Path,
) -> anyhow::Result<String> {
    let arguments = gateway_arguments(spec)?
        .iter()
        .map(|argument| format!("    <string>{}</string>\n", xml_escape(argument)))
        .collect::<String>();
    let working_directory = xml_escape(utf8_path("working directory", working_directory)?);
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
  <key>Label</key>\n  <string>{LAUNCHD_LABEL}</string>\n\
  <key>ProgramArguments</key>\n  <array>\n{arguments}  </array>\n\
  <key>RunAtLoad</key>\n  <true/>\n\
  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n\
  <key>ThrottleInterval</key>\n  <integer>{LAUNCHD_THROTTLE_SECONDS}</integer>\n\
  <key>ProcessType</key>\n  <string>Background</string>\n\
  <key>StandardOutPath</key>\n  <string>/dev/null</string>\n\
  <key>StandardErrorPath</key>\n  <string>/dev/null</string>\n\
  <key>WorkingDirectory</key>\n  <string>{working_directory}</string>\n\
</dict>\n</plist>\n"
    ))
}

/// Inspect the semantic content of a LaunchAgent (as converted to JSON by
/// `plutil`). Returns the program it runs when every other field matches the
/// definition this CLI installs; formatting and key order written by older
/// app versions do not matter. The caller decides whether that program is
/// acceptable, so the bundled CLI and an installed CLI copy of the same
/// version do not keep replacing each other's definition.
pub(super) fn launchd_program(
    plist: &serde_json::Value,
    spec: &StartupServiceSpec<'_>,
) -> anyhow::Result<Option<String>> {
    let [_, start, log_flag, log_file] = gateway_arguments(spec)?;
    let Some([program, rest @ ..]) = plist["ProgramArguments"].as_array().map(Vec::as_slice) else {
        return Ok(None);
    };
    let arguments_match = rest.len() == 3
        && rest
            .iter()
            .zip([start, log_flag, log_file])
            .all(|(actual, expected)| actual.as_str() == Some(expected));
    let matches = arguments_match
        && plist["Label"].as_str() == Some(LAUNCHD_LABEL)
        && plist["RunAtLoad"].as_bool() == Some(true)
        && plist["KeepAlive"]["SuccessfulExit"].as_bool() == Some(false)
        && plist["ThrottleInterval"].as_i64() == Some(LAUNCHD_THROTTLE_SECONDS)
        && plist["ProcessType"].as_str() == Some("Background");
    Ok(matches
        .then(|| program.as_str().map(str::to_owned))
        .flatten())
}

// ---------------------------------------------------------------- systemd

/// Quote one ExecStart argument. systemd expands `%` specifiers and `$`
/// environment references even inside double quotes, so both are doubled.
fn systemd_exec_argument(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '%' => quoted.push_str("%%"),
            '$' => quoted.push_str("$$"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

pub(super) fn render_systemd_unit(spec: &StartupServiceSpec<'_>) -> anyhow::Result<String> {
    let exec_start = gateway_arguments(spec)?
        .iter()
        .map(|argument| systemd_exec_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!(
        "[Unit]\nDescription=Codex Mixin Gateway\nAfter=network-online.target\n\n\
[Service]\nType=simple\nExecStart={exec_start}\nRestart=on-failure\nRestartSec=10\n\
WorkingDirectory=%h\n\n[Install]\nWantedBy=default.target\n"
    ))
}

/// Interpret `systemctl --user is-active` stdout, which is a fixed
/// machine-readable state word rather than localized prose.
pub(super) fn systemd_active_state(stdout: &str) -> Option<bool> {
    match stdout.lines().next().map(str::trim)? {
        "active" | "activating" | "reloading" | "deactivating" | "refreshing" => Some(true),
        "inactive" | "failed" => Some(false),
        _ => None,
    }
}

/// Interpret `systemctl --user is-enabled` stdout.
pub(super) fn systemd_enabled_state(stdout: &str) -> Option<bool> {
    match stdout.lines().next().map(str::trim)? {
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "alias" => Some(true),
        "disabled" | "masked" | "masked-runtime" | "static" | "indirect" | "generated"
        | "transient" | "bad" => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------------------- Windows

/// Quote one argument for the Windows command-line parser used by the Rust
/// runtime. Paths cannot contain `"`, so rejecting it keeps quoting simple.
fn windows_argument(value: &str) -> anyhow::Result<String> {
    if value.contains('"') {
        bail!("Windows startup path contains a double quote");
    }
    let trailing_backslashes = value.len() - value.trim_end_matches('\\').len();
    Ok(format!("\"{value}{}\"", "\\".repeat(trailing_backslashes)))
}

/// The scheduled task runs the gateway under a headless console host so the
/// console-subsystem CLI does not open a window at every logon.
pub(super) fn windows_task_arguments(spec: &StartupServiceSpec<'_>) -> anyhow::Result<String> {
    let [executable, start, log_flag, log_file] = gateway_arguments(spec)?;
    Ok(format!(
        "--headless {} {start} {log_flag} {}",
        windows_argument(executable)?,
        windows_argument(log_file)?
    ))
}

/// Structured scheduled-task fields returned by `Get-ScheduledTask`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct WindowsTaskInfo {
    pub(super) state: String,
    pub(super) enabled: bool,
    pub(super) action_count: u32,
    pub(super) execute: String,
    pub(super) arguments: String,
    pub(super) run_level: String,
    pub(super) logon_type: String,
    pub(super) user_id: String,
    pub(super) current_user: String,
    pub(super) logon_trigger: bool,
}

pub(super) fn windows_task_is_current(
    task: &WindowsTaskInfo,
    console_host: &Path,
    spec: &StartupServiceSpec<'_>,
) -> anyhow::Result<bool> {
    let console_host = utf8_path("console host", console_host)?;
    let same_user = task.user_id.eq_ignore_ascii_case(&task.current_user)
        || task
            .current_user
            .rsplit('\\')
            .next()
            .is_some_and(|name| task.user_id.eq_ignore_ascii_case(name));
    Ok(task.action_count == 1
        && task.execute.eq_ignore_ascii_case(console_host)
        && task.arguments == windows_task_arguments(spec)?
        && task.run_level == "Limited"
        && task.logon_type == "Interactive"
        && task.logon_trigger
        && same_user)
}

pub(super) fn windows_task_is_running(task: &WindowsTaskInfo) -> bool {
    task.state == "Running"
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn spec<'a>(executable: &'a Path, log_file: &'a Path) -> StartupServiceSpec<'a> {
        StartupServiceSpec {
            executable,
            log_file,
        }
    }

    #[test]
    fn launchd_plist_escapes_xml_values() {
        let executable = PathBuf::from("/Apps/A&B <x>/codex-mixin");
        let log = PathBuf::from("/Users/o'neil/.codex-mixin/gateway.log");
        let plist =
            render_launchd_plist(&spec(&executable, &log), Path::new("/Users/o'neil")).unwrap();
        assert!(plist.contains("<string>/Apps/A&amp;B &lt;x&gt;/codex-mixin</string>"));
        assert!(plist.contains("<string>/Users/o&apos;neil/.codex-mixin/gateway.log</string>"));
    }

    fn launchd_json(executable: &str, log: &str) -> serde_json::Value {
        serde_json::json!({
            "Label": LAUNCHD_LABEL,
            "ProgramArguments": [executable, "start", "--log-file", log],
            "RunAtLoad": true,
            "KeepAlive": {"SuccessfulExit": false},
            "ThrottleInterval": 10,
            "ProcessType": "Background",
            "StandardOutPath": "/dev/null",
        })
    }

    #[test]
    fn launchd_definition_matches_semantics_and_detects_drift() {
        let executable =
            PathBuf::from("/Applications/Codex Mixin.app/Contents/Resources/codex-mixin");
        let log = PathBuf::from("/Users/me/.codex-mixin/gateway.log");
        let spec = spec(&executable, &log);
        let current = launchd_json(executable.to_str().unwrap(), log.to_str().unwrap());
        assert_eq!(
            launchd_program(&current, &spec).unwrap().as_deref(),
            executable.to_str()
        );

        let other_copy = launchd_json("/Users/me/.local/bin/codex-mixin", log.to_str().unwrap());
        assert_eq!(
            launchd_program(&other_copy, &spec).unwrap().as_deref(),
            Some("/Users/me/.local/bin/codex-mixin")
        );

        let other_log = launchd_json(executable.to_str().unwrap(), "/tmp/elsewhere.log");
        assert_eq!(launchd_program(&other_log, &spec).unwrap(), None);

        let mut no_keepalive = current.clone();
        no_keepalive["KeepAlive"] = serde_json::json!(true);
        assert_eq!(launchd_program(&no_keepalive, &spec).unwrap(), None);

        let mut not_at_load = current;
        not_at_load["RunAtLoad"] = serde_json::json!(false);
        assert_eq!(launchd_program(&not_at_load, &spec).unwrap(), None);
    }

    #[test]
    fn systemd_unit_escapes_specifiers_environment_and_quotes() {
        let executable = PathBuf::from("/home/me/100% \"tools\"/$HOME\\bin/codex-mixin");
        let log = PathBuf::from("/home/me/.codex-mixin/gateway.log");
        let unit = render_systemd_unit(&spec(&executable, &log)).unwrap();
        let exec = unit
            .lines()
            .find_map(|line| line.strip_prefix("ExecStart="))
            .unwrap();
        assert_eq!(
            exec,
            "\"/home/me/100%% \\\"tools\\\"/$$HOME\\\\bin/codex-mixin\" \"start\" \
\"--log-file\" \"/home/me/.codex-mixin/gateway.log\""
        );
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn service_definitions_reject_line_breaks_in_paths() {
        let executable = PathBuf::from("/home/me/bad\nExecStartPre=/bin/sh");
        let log = PathBuf::from("/tmp/gateway.log");
        assert!(render_systemd_unit(&spec(&executable, &log)).is_err());
        assert!(render_launchd_plist(&spec(&executable, &log), Path::new("/")).is_err());
        assert!(windows_task_arguments(&spec(&executable, &log)).is_err());
    }

    #[test]
    fn systemctl_state_words_are_classified_and_unknown_output_is_an_error() {
        assert_eq!(systemd_active_state("active\n"), Some(true));
        assert_eq!(systemd_active_state("activating\n"), Some(true));
        assert_eq!(systemd_active_state("inactive\n"), Some(false));
        assert_eq!(systemd_active_state("failed\n"), Some(false));
        assert_eq!(systemd_active_state(""), None);
        assert_eq!(systemd_enabled_state("enabled\n"), Some(true));
        assert_eq!(systemd_enabled_state("disabled\n"), Some(false));
        assert_eq!(systemd_enabled_state("Failed to connect to bus"), None);
    }

    #[test]
    fn windows_task_arguments_quote_paths_for_the_headless_host() {
        let executable = PathBuf::from(r"C:\Users\Me Too\.codex-mixin\bin\codex-mixin.exe");
        let log = PathBuf::from(r"C:\Users\Me Too\.codex-mixin\gateway.log");
        assert_eq!(
            windows_task_arguments(&spec(&executable, &log)).unwrap(),
            r#"--headless "C:\Users\Me Too\.codex-mixin\bin\codex-mixin.exe" start --log-file "C:\Users\Me Too\.codex-mixin\gateway.log""#
        );
        assert_eq!(windows_argument(r"C:\dir\").unwrap(), r#""C:\dir\\""#);
        assert!(windows_argument("C:\\a\"b").is_err());
    }

    fn task(arguments: String) -> WindowsTaskInfo {
        WindowsTaskInfo {
            state: "Ready".to_owned(),
            enabled: true,
            action_count: 1,
            execute: r"C:\WINDOWS\System32\conhost.exe".to_owned(),
            arguments,
            run_level: "Limited".to_owned(),
            logon_type: "Interactive".to_owned(),
            user_id: "me".to_owned(),
            current_user: r"HOST\me".to_owned(),
            logon_trigger: true,
        }
    }

    #[test]
    fn windows_task_match_requires_current_user_limited_logon_definition() {
        let executable = PathBuf::from(r"C:\bin\codex-mixin.exe");
        let log = PathBuf::from(r"C:\state\gateway.log");
        let spec = spec(&executable, &log);
        let host = PathBuf::from(r"C:\Windows\System32\conhost.exe");
        let arguments = windows_task_arguments(&spec).unwrap();
        assert!(windows_task_is_current(&task(arguments.clone()), &host, &spec).unwrap());

        let mut elevated = task(arguments.clone());
        elevated.run_level = "Highest".to_owned();
        assert!(!windows_task_is_current(&elevated, &host, &spec).unwrap());

        let mut other_user = task(arguments.clone());
        other_user.user_id = "someone".to_owned();
        assert!(!windows_task_is_current(&other_user, &host, &spec).unwrap());

        let stale = task(arguments.replace("codex-mixin.exe", "old.exe"));
        assert!(!windows_task_is_current(&stale, &host, &spec).unwrap());

        let decoded: WindowsTaskInfo = serde_json::from_str(
            r#"{"State":"Running","Enabled":true,"ActionCount":1,"Execute":"x","Arguments":"y",
                "RunLevel":"Limited","LogonType":"Interactive","UserId":"me",
                "CurrentUser":"HOST\\me","LogonTrigger":true}"#,
        )
        .unwrap();
        assert!(windows_task_is_running(&decoded));
    }
}
