param(
    [Parameter(Mandatory = $true)][string]$CliPath,
    [switch]$RestoreAutostart,
    [switch]$RepairLegacyTask
)

$ErrorActionPreference = 'Stop'

try {
    if ($RepairLegacyTask) {
        # Only the normal installer's task can be repaired. An isolated service
        # must never make changes to the real user's gateway task.
        if ($env:CODEX_GATEWAY_SERVICE_ID) { exit 1 }
        $userSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        $repair = @'
$ErrorActionPreference = 'Stop'
try {
    $requester = [Security.Principal.SecurityIdentifier]::new('__REQUESTER_SID__')
    $newCli = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__CLI_PATH__'))
    if (![IO.Path]::IsPathRooted($newCli) -or [IO.Path]::GetFileName($newCli) -ne 'codex-mixin.exe' -or
        $newCli -match '[\r\n\x00]' -or !(Test-Path -LiteralPath $newCli -PathType Leaf)) {
        throw 'Invalid replacement gateway executable.'
    }
    $scheduler = New-Object -ComObject 'Schedule.Service'
    $scheduler.Connect()
    $folder = $scheduler.GetFolder('\')
    $task = $folder.GetTask('Codex Mixin Gateway')
    $definition = $task.Definition
    $principal = $definition.Principal
    $registeredUser = if ($principal.UserId -match '^S-1-') {
        [Security.Principal.SecurityIdentifier]::new($principal.UserId)
    } else {
        [Security.Principal.NTAccount]::new($principal.UserId).Translate(
            [Security.Principal.SecurityIdentifier]
        )
    }
    if (!$requester.Equals($registeredUser) -or $principal.LogonType -ne 3 -or
        $principal.RunLevel -ne 0 -or $definition.Actions.Count -ne 1) {
        throw 'The existing gateway task does not belong to the current limited user.'
    }
    $action = $definition.Actions.Item(1)
    $console = Join-Path $env:SystemRoot 'System32\conhost.exe'
    $launcher = [regex]::Match($action.Arguments, '^(--headless .+ -EncodedCommand )([A-Za-z0-9+/=]+)$')
    if ($action.Type -ne 0 -or $action.Path -ne $console -or !$launcher.Success) {
        throw 'The existing gateway task has an unrecognized launcher.'
    }
    $body = [Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($launcher.Groups[2].Value))
    $oldCli = [regex]::Match($body, "(?<=; & )'(?<exe>(?:[^']|'')+\\codex-mixin\.exe)'(?= 'start' '--log-file' )",
        [Text.RegularExpressions.RegexOptions]::IgnoreCase)
    if (!$oldCli.Success) { throw 'The existing task does not launch the Codex Mixin gateway.' }
    $quotedCli = "'" + $newCli.Replace("'", "''") + "'"
    $body = $body.Remove($oldCli.Index, $oldCli.Length).Insert($oldCli.Index, $quotedCli)
    $action.Arguments = $launcher.Groups[1].Value + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($body))

    # Give the existing limited task's own principal permission to manage it.
    # Preserve the descriptor's other entries, trigger and limited identity.
    $sections = [Security.AccessControl.AccessControlSections]::Access
    # Legacy tasks can store inherited entries ahead of explicit ones.
    # Append the grant without reordering or replacing their existing entries.
    $acl = [Security.AccessControl.RawSecurityDescriptor]::new($task.GetSecurityDescriptor(4))
    $dacl = $acl.DiscretionaryAcl
    if ($null -eq $dacl) { throw 'The task has no discretionary access list to repair.' }
    $grant = [Security.AccessControl.CommonAce]::new(
        [Security.AccessControl.AceFlags]::None,
        [Security.AccessControl.AceQualifier]::AccessAllowed,
        0x1f01ff, $requester, $false, $null
    )
    $alreadyGranted = $dacl | Where-Object {
        $_ -is [Security.AccessControl.CommonAce] -and $_.SecurityIdentifier.Equals($requester) -and
        $_.AceQualifier -eq $grant.AceQualifier -and ($_.AccessMask -band $grant.AccessMask) -eq $grant.AccessMask
    }
    if (!$alreadyGranted) { $dacl.InsertAce($dacl.Count, $grant) }

    if ($task.State -eq 4) { $task.Stop(0) }
    # TASK_UPDATE | TASK_DONT_ADD_PRINCIPAL_ACE | TASK_IGNORE_REGISTRATION_TRIGGERS.
    # Update both the command and permissions before returning to the normal CLI.
    $null = $folder.RegisterTaskDefinition($task.Name, $definition, 0x34,
        $principal.UserId, $null, 3, $acl.GetSddlForm($sections))
    exit 0
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
'@
        $encodedPath = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($CliPath))
        $repair = $repair.Replace('__REQUESTER_SID__', $userSid).Replace('__CLI_PATH__', $encodedPath)
        $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($repair))
        $powershell = Join-Path ([Environment]::SystemDirectory) 'WindowsPowerShell\v1.0\powershell.exe'
        $process = Start-Process -FilePath $powershell -Verb RunAs -WindowStyle Hidden -Wait -PassThru `
            -ArgumentList @('-NoProfile', '-NonInteractive', '-EncodedCommand', $encoded)
        if ($process.ExitCode -ne 0) {
            [Console]::Error.WriteLine("The elevated task repair failed (exit $($process.ExitCode)).")
            exit 1
        }
    }

    # Windows PowerShell wraps native stderr in ErrorRecords. Preserve the
    # native exit status and inspect the HRESULT before reporting the failure.
    $ErrorActionPreference = 'Continue'
    $gatewayArgs = if ($RestoreAutostart) {
        @('autostart', 'enable', '--json')
    } else {
        @('start', '--managed', '--json')
    }
    $result = & $CliPath --no-tui --json-errors service @gatewayArgs 2>&1
    $status = $LASTEXITCODE
    if ($status -eq 0) { exit 0 }
    $diagnostic = $result | Out-String
    [Console]::Error.WriteLine($diagnostic.Trim())
    if (!$env:CODEX_GATEWAY_SERVICE_ID -and
        $diagnostic -match '(?:register|start|stop) gateway scheduled task failed' -and
        $diagnostic -match '0x80070005') { exit 5 }
    exit 1
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
