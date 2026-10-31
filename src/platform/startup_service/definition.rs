//! Pure rendering and inspection of OS startup-service definitions. These
//! functions compile on every platform so their escaping and matching rules
//! are tested everywhere, independent of which adapter is active.

use std::path::Path;

use anyhow::bail;
use serde::Deserialize;

const LAUNCHD_LABEL: &str = "local.codex-mixin.service";
pub(super) const LAUNCHD_THROTTLE_SECONDS: i64 = 10;
const SYSTEMD_UNIT_NAME: &str = "codex-mixin.service";
const WINDOWS_TASK_NAME: &str = "Codex Mixin Gateway";

/// Names an isolated service identity. When set, the launchd label, systemd
/// unit and Windows task are suffixed with it so an isolated instance (tests,
/// sandboxes) can never address the user's real startup service. Unset keeps
/// the default identity.
pub const SERVICE_ID_ENV: &str = "CODEX_GATEWAY_SERVICE_ID";
const MAX_SERVICE_ID_LEN: usize = 64;

fn service_id() -> anyhow::Result<Option<String>> {
    parse_service_id(std::env::var_os(SERVICE_ID_ENV))
}

fn parse_service_id(raw: Option<std::ffi::OsString>) -> anyhow::Result<Option<String>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let id = raw
        .into_string()
        .map_err(|_| anyhow::anyhow!("{SERVICE_ID_ENV} is not valid UTF-8"))?;
    let valid = !id.is_empty()
        && id.len() <= MAX_SERVICE_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    anyhow::ensure!(
        valid,
        "{SERVICE_ID_ENV} must be 1-{MAX_SERVICE_ID_LEN} ASCII letters, digits, '-' or '_'"
    );
    Ok(Some(id))
}

fn scoped_launchd_label(id: Option<&str>) -> String {
    id.map_or_else(
        || LAUNCHD_LABEL.to_owned(),
        |id| format!("local.codex-mixin.{id}.service"),
    )
}

fn scoped_systemd_unit(id: Option<&str>) -> String {
    id.map_or_else(
        || SYSTEMD_UNIT_NAME.to_owned(),
        |id| format!("codex-mixin-{id}.service"),
    )
}

fn scoped_windows_task(id: Option<&str>) -> String {
    id.map_or_else(
        || WINDOWS_TASK_NAME.to_owned(),
        |id| format!("{WINDOWS_TASK_NAME} {id}"),
    )
}

pub(super) fn launchd_label() -> anyhow::Result<String> {
    Ok(scoped_launchd_label(service_id()?.as_deref()))
}

pub(super) fn systemd_unit_name() -> anyhow::Result<String> {
    Ok(scoped_systemd_unit(service_id()?.as_deref()))
}

pub(super) fn windows_task_name() -> anyhow::Result<String> {
    Ok(scoped_windows_task(service_id()?.as_deref()))
}

/// Command line of the supervised gateway process.
#[derive(Clone, Copy, Debug)]
pub struct StartupServiceSpec<'a> {
    pub executable: &'a Path,
    pub log_file: &'a Path,
    pub config_path: &'a Path,
    pub codex_home: &'a Path,
}

fn gateway_environment<'a>(
    spec: &StartupServiceSpec<'a>,
) -> anyhow::Result<[(&'static str, &'a str); 2]> {
    Ok([
        (
            "CODEX_GATEWAY_CONFIG",
            utf8_path("gateway config", spec.config_path)?,
        ),
        ("CODEX_HOME", utf8_path("Codex home", spec.codex_home)?),
    ])
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
    let label = launchd_label()?;
    let environment = gateway_environment(spec)?
        .iter()
        .map(|(name, value)| {
            format!(
                "    <key>{name}</key>\n    <string>{}</string>\n",
                xml_escape(value)
            )
        })
        .collect::<String>();
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
  <key>Label</key>\n  <string>{label}</string>\n\
  <key>ProgramArguments</key>\n  <array>\n{arguments}  </array>\n\
  <key>EnvironmentVariables</key>\n  <dict>\n{environment}  </dict>\n\
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
    let environment_matches = gateway_environment(spec)?
        .iter()
        .all(|(name, value)| plist["EnvironmentVariables"][name].as_str() == Some(*value));
    let matches = arguments_match
        && environment_matches
        && plist["Label"].as_str() == Some(launchd_label()?.as_str())
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
    // Environment= expands percent specifiers, but dollar signs are literal.
    let environment = gateway_environment(spec)?
        .iter()
        .map(|(name, value)| {
            let escaped = value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%");
            format!("Environment=\"{name}={escaped}\"\n")
        })
        .collect::<String>();
    Ok(format!(
        "[Unit]\nDescription=Codex Mixin Gateway\nAfter=network-online.target\n\n\
[Service]\nType=simple\n{environment}ExecStart={exec_start}\nRestart=on-failure\nRestartSec=10\n\
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

/// The scheduled task runs the gateway under a headless console host so the
/// console-subsystem CLI does not open a window at every logon. PowerShell
/// supplies the persisted environment; UTF-16 encoding avoids shell expansion.
pub(super) fn windows_task_arguments(spec: &StartupServiceSpec<'_>) -> anyhow::Result<String> {
    let mut script = String::from("$ErrorActionPreference = 'Stop'; ");
    for (name, value) in gateway_environment(spec)? {
        script.push_str(&format!(
            "$env:{name} = {}; ",
            crate::platform::shell::powershell_quote(value)
        ));
    }
    script.push_str("& ");
    for argument in gateway_arguments(spec)? {
        script.push_str(&crate::platform::shell::powershell_quote(argument));
        script.push(' ');
    }
    script.push_str("; exit $LASTEXITCODE");
    Ok(format!(
        "--headless {}",
        crate::platform::shell::encoded_command(&script)
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
