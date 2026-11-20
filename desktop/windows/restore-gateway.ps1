param(
    [Parameter(Mandatory = $true)][string]$CliPath,
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
    $scheduler = New-Object -ComObject 'Schedule.Service'
    $scheduler.Connect()
    $task = $scheduler.GetFolder('\').GetTask('Codex Mixin Gateway')
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
        $principal.RunLevel -ne 0 -or $definition.Actions.Count -ne 1) { exit 1 }
    $action = $definition.Actions.Item(1)
    $console = Join-Path ([Environment]::SystemDirectory) 'conhost.exe'
    if ($action.Type -ne 0 -or $action.Path -ne $console -or
        $action.Arguments -notlike '--headless *') { exit 1 }

    # Give the existing limited task's own principal permission to manage it.
    # Preserve the descriptor's other entries and the task's action and identity.
    $sections = [Security.AccessControl.AccessControlSections]::Access
    $acl = [Security.AccessControl.CommonSecurityDescriptor]::new(
        $false, $false, $task.GetSecurityDescriptor(4)
    )
    $acl.DiscretionaryAcl.AddAccess(
        [Security.AccessControl.AccessControlType]::Allow, $requester, 0x1f01ff,
        [Security.AccessControl.InheritanceFlags]::None,
        [Security.AccessControl.PropagationFlags]::None
    )
    $task.SetSecurityDescriptor($acl.GetSddlForm($sections), 0x10)
    exit 0
} catch { exit 1 }
'@
        $repair = $repair.Replace('__REQUESTER_SID__', $userSid)
        $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($repair))
        $powershell = Join-Path ([Environment]::SystemDirectory) 'WindowsPowerShell\v1.0\powershell.exe'
        $process = Start-Process -FilePath $powershell -Verb RunAs -WindowStyle Hidden -Wait -PassThru `
            -ArgumentList @('-NoProfile', '-NonInteractive', '-EncodedCommand', $encoded)
        if ($process.ExitCode -ne 0) { exit 1 }
    }

    # Windows PowerShell wraps native stderr in ErrorRecords. Preserve the
    # native exit status and inspect the HRESULT before reporting the failure.
    $ErrorActionPreference = 'Continue'
    $result = & $CliPath --no-tui --json-errors service start --managed --json 2>&1
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
