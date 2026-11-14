use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, bail};

use super::definition::{
    WindowsTaskInfo, windows_task_arguments, windows_task_is_current, windows_task_is_running,
    windows_task_name,
};
use super::{StartupServiceSpec, StartupServiceStatus};

pub(super) const SUPPORTED: bool = true;

// Values reach PowerShell only through environment variables, never through
// script interpolation, so paths cannot inject commands.
const INSTALL_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$action = New-ScheduledTaskAction -Execute $env:CODEX_MIXIN_TASK_EXECUTE -Argument $env:CODEX_MIXIN_TASK_ARGUMENTS
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew -StartWhenAvailable -RestartCount 3 -RestartInterval ([TimeSpan]::FromMinutes(1))
Register-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\' -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
"#;

const CONTROL_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
switch ($env:CODEX_MIXIN_TASK_OPERATION) {
  'start' { Start-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\' }
  'stop' {
    $task = Get-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\'
    if ($task.State -eq 'Running') { Stop-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\' }
  }
  'remove' {
    $task = Get-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\'
    if ($task.State -eq 'Running') { Stop-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\' }
    Unregister-ScheduledTask -TaskName $env:CODEX_MIXIN_TASK_NAME -TaskPath '\' -Confirm:$false
  }
}
"#;

fn powershell(script: &str, environment: &[(&str, &str)]) -> anyhow::Result<Output> {
    let mut command = Command::new(crate::system_powershell()?);
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
        .env("CODEX_MIXIN_TASK_NAME", windows_task_name()?);
    for (key, value) in environment {
        command.env(key, value);
    }
    crate::prepare_background_command(&mut command);
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
    use winsafe::{IExecAction, ITaskService, IUnknown, Variant, co, prelude::*};

    // Keep the apartment alive until every COM interface has been released on
    // this blocking thread. Querying must not start a PowerShell process.
    let _apartment = winsafe::CoInitializeEx(co::COINIT::MULTITHREADED)?;
    let service: ITaskService = winsafe::CoCreateInstance(
        &co::CLSID::TaskScheduler,
        None::<&IUnknown>,
        co::CLSCTX::INPROC_SERVER,
    )?;
    service.Connect(None, None, None, None)?;
    let folder = service.GetFolder("\\")?;
    let task = match folder.GetTask(&windows_task_name()?) {
        Ok(task) => task,
        Err(error) if error == co::ERROR::FILE_NOT_FOUND.to_hresult() => return Ok(None),
        Err(error) => return Err(error).context("query gateway scheduled task"),
    };
    let definition = task.get_Definition()?;
    let settings = definition.get_Settings()?;
    let actions = definition.get_Actions()?;
    let action_count = u32::try_from(actions.get_Count()?)?;
    let mut execute = String::new();
    let mut arguments = String::new();
    if action_count > 0 {
        let action = actions.get_Item(1)?;
        if action.get_Type()? == co::TASK_ACTION_TYPE::EXEC {
            let action: IExecAction = action.QueryInterface()?;
            execute = action.get_Path()?;
            arguments = action.get_Arguments()?;
        }
    }
    let principal = definition.get_Principal()?;
    // The wrapper exposes these Automation properties through IDispatch.
    let Variant::I4(logon_type) = principal
        .invoke_get("LogonType", &[])
        .map_err(anyhow::Error::from_boxed)?
    else {
        bail!("scheduled task LogonType is not an integer");
    };
    let Variant::Bstr(user_id) = principal
        .invoke_get("UserId", &[])
        .map_err(anyhow::Error::from_boxed)?
    else {
        bail!("scheduled task UserId is not a string");
    };
    let triggers = definition.get_Triggers()?;
    let mut logon_trigger = false;
    for index in 1..=triggers.get_Count()? {
        if triggers.get_Item(index)?.get_Type()? == co::TASK_TRIGGER_TYPE2::LOGON {
            logon_trigger = true;
            break;
        }
    }
    let user = service.get_ConnectedUser()?;
    let domain = service.get_ConnectedDomain()?;
    let current_user = if user.contains('\\') || domain.is_empty() {
        user
    } else {
        format!("{domain}\\{user}")
    };
    Ok(Some(WindowsTaskInfo {
        state: if task.get_State()? == co::TASK_STATE::RUNNING {
            "Running"
        } else {
            "NotRunning"
        }
        .into(),
        enabled: task.get_Enabled()?,
        action_count,
        execute,
        arguments,
        run_level: if principal.get_RunLevel()? == co::TASK_RUNLEVEL::LUA {
            "Limited"
        } else {
            "Highest"
        }
        .into(),
        logon_type: if logon_type == co::TASK_LOGON::INTERACTIVE_TOKEN.raw() as i32 {
            "Interactive"
        } else {
            "NonInteractive"
        }
        .into(),
        user_id,
        current_user,
        logon_trigger,
        start_when_available: settings.get_StartWhenAvailable()?,
        restart_count: settings.get_RestartCount()?,
        restart_interval: settings.get_RestartInterval()?,
    }))
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
