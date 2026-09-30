use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, bail};

use super::definition::{
    WINDOWS_TASK_NAME, WindowsTaskInfo, windows_task_arguments, windows_task_is_current,
    windows_task_is_running,
};
use super::{StartupServiceSpec, StartupServiceStatus};

pub(super) const SUPPORTED: bool = true;

// Values reach PowerShell only through environment variables, never through
// script interpolation, so paths cannot inject commands.
const QUERY_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$task = Get-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\' -ErrorAction SilentlyContinue
if ($null -eq $task) { 'null'; exit 0 }
$action = @($task.Actions)[0]
[pscustomobject]@{
  State = [string]$task.State
  Enabled = [bool]$task.Settings.Enabled
  ActionCount = @($task.Actions).Count
  Execute = [string]$action.Execute
  Arguments = [string]$action.Arguments
  RunLevel = [string]$task.Principal.RunLevel
  LogonType = [string]$task.Principal.LogonType
  UserId = [string]$task.Principal.UserId
  CurrentUser = [Security.Principal.WindowsIdentity]::GetCurrent().Name
  LogonTrigger = [bool](@($task.Triggers | Where-Object { $_.CimClass.CimClassName -eq 'MSFT_TaskLogonTrigger' }).Count -gt 0)
} | ConvertTo-Json -Compress
"#;

const INSTALL_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$action = New-ScheduledTaskAction -Execute $env:CODEX_MIXIN_TASK_EXECUTE -Argument $env:CODEX_MIXIN_TASK_ARGUMENTS
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\' -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
"#;

const CONTROL_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
switch ($env:CODEX_MIXIN_TASK_OPERATION) {
  'start' { Start-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\' }
  'stop' { Stop-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\' }
  'remove' {
    Stop-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\'
    Unregister-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\' -Confirm:$false
  }
}
"#;

fn powershell(script: &str, environment: &[(&str, &str)]) -> anyhow::Result<Output> {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .env("CODEX_MIXIN_TASK_NAME", WINDOWS_TASK_NAME);
    for (key, value) in environment {
        command.env(key, value);
    }
    crate::platform::prepare_background_command(&mut command);
    command.output().context("run PowerShell ScheduledTasks")
}

fn checked(script: &str, environment: &[(&str, &str)], operation: &str) -> anyhow::Result<Output> {
    let output = powershell(script, environment)?;
    if output.status.success() {
        return Ok(output);
    }
    bail!(
        "{operation} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn console_host() -> anyhow::Result<PathBuf> {
    let root = std::env::var_os("SystemRoot").context("SystemRoot is not set")?;
    Ok(PathBuf::from(root).join("System32").join("conhost.exe"))
}

fn query() -> anyhow::Result<Option<WindowsTaskInfo>> {
    let output = checked(QUERY_SCRIPT, &[], "query gateway scheduled task")?;
    let stdout = String::from_utf8(output.stdout).context("decode scheduled task query")?;
    let stdout = stdout.trim().trim_start_matches('\u{feff}');
    if stdout == "null" {
        return Ok(None);
    }
    serde_json::from_str(stdout)
        .map(Some)
        .context("decode scheduled task query JSON")
}

pub(super) fn status(spec: &StartupServiceSpec<'_>) -> anyhow::Result<StartupServiceStatus> {
    let Some(task) = query()? else {
        return Ok(StartupServiceStatus::default());
    };
    Ok(StartupServiceStatus {
        installed: task.enabled,
        current: task.enabled && windows_task_is_current(&task, &console_host()?, spec)?,
        loaded: windows_task_is_running(&task),
    })
}

pub(super) fn install(spec: &StartupServiceSpec<'_>) -> anyhow::Result<()> {
    let host = console_host()?;
    let host = host
        .to_str()
        .context("console host path is not valid UTF-8")?;
    let arguments = windows_task_arguments(spec)?;
    checked(
        INSTALL_SCRIPT,
        &[
            ("CODEX_MIXIN_TASK_EXECUTE", host),
            ("CODEX_MIXIN_TASK_ARGUMENTS", &arguments),
        ],
        "register gateway scheduled task",
    )
    .map(drop)
}

fn control(operation: &str) -> anyhow::Result<()> {
    checked(
        CONTROL_SCRIPT,
        &[("CODEX_MIXIN_TASK_OPERATION", operation)],
        &format!("{operation} gateway scheduled task"),
    )
    .map(drop)
}

pub(super) fn start() -> anyhow::Result<()> {
    control("start")
}

pub(super) fn stop() -> anyhow::Result<()> {
    if query()?.is_none() {
        return Ok(());
    }
    control("stop")
}

pub(super) fn remove() -> anyhow::Result<()> {
    if query()?.is_none() {
        return Ok(());
    }
    control("remove")
}
