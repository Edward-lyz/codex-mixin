param([Parameter(Mandatory = $true)][string]$Installer)

$ErrorActionPreference = 'Stop'
$Installer = (Resolve-Path -LiteralPath $Installer).Path
$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexMixin'
foreach ($view in @([Microsoft.Win32.RegistryView]::Registry64, [Microsoft.Win32.RegistryView]::Registry32)) {
    $registry = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, $view)
    try {
        $existing = $registry.OpenSubKey($uninstallKey)
        if ($existing) { $existing.Dispose(); throw 'Gateway installer test requires a clean CI runner.' }
    } finally { $registry.Dispose() }
}
$runnerRoot = (Resolve-Path -LiteralPath $env:RUNNER_TEMP).Path.TrimEnd([IO.Path]::DirectorySeparatorChar)
$fixture = [IO.Path]::GetFullPath((Join-Path $runnerRoot ('codex-mixin-gateway-install-' + [Guid]::NewGuid().ToString('N'))))
if (-not $fixture.StartsWith($runnerRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Fixture escaped RUNNER_TEMP.' }
$directory = Join-Path $fixture 'app'
$identity = 'install-' + [Guid]::NewGuid().ToString('N')
$taskName = 'Codex Mixin Gateway ' + $identity
$previousEnvironment = @{}
$isolated = @{
    USERPROFILE = (Join-Path $fixture 'home')
    CODEX_HOME = (Join-Path $fixture 'codex')
    CODEX_GATEWAY_CONFIG = (Join-Path $fixture 'config.json')
    CODEX_GATEWAY_RUNTIME_FILE = (Join-Path $fixture 'runtime.json')
    CODEX_GATEWAY_DAEMON_FILE = (Join-Path $fixture 'daemon.json')
    CODEX_GATEWAY_SERVICE_ID = $identity
    APPDATA = (Join-Path $fixture 'appdata')
    LOCALAPPDATA = (Join-Path $fixture 'localappdata')
    HTTP_PROXY = 'http://127.0.0.1:9'; HTTPS_PROXY = 'http://127.0.0.1:9'; NO_PROXY = '127.0.0.1,localhost'
}
foreach ($entry in $isolated.GetEnumerator()) {
    $previousEnvironment[$entry.Key] = [Environment]::GetEnvironmentVariable($entry.Key)
    [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
}
$upstream = $null
$runKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Run')
$previousStartup = if ($runKey) { $runKey.GetValue('CodexMixin') } else { $null }
if ($runKey) { $runKey.Dispose() }
New-Item -ItemType Directory -Force -Path $fixture, $isolated.USERPROFILE, $isolated.CODEX_HOME | Out-Null
$cli = Join-Path $directory 'codex-mixin.exe'
function Invoke-Installer {
    $process = Start-Process -FilePath $Installer -ArgumentList @('/S', "/D=$directory") -Wait -PassThru -WindowStyle Hidden
    if ($process.ExitCode -ne 0) { throw "Installer failed (exit $($process.ExitCode))." }
}
function Assert-Gateway([string]$Expected) {
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    do {
        $status = & $cli --no-tui --json-errors service status --json | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { throw 'Gateway status query failed.' }
        if ($status.gateway -eq $Expected) { return }
        Start-Sleep -Milliseconds 300
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Gateway did not reach $Expected after installation or task start."
}
try {
    $ready = Join-Path $fixture 'upstream.port'
    $mock = (Resolve-Path (Join-Path $PSScriptRoot '..\..\scripts\e2e_mock_provider.mjs')).Path
    $node = (Get-Command node -ErrorAction Stop).Source
    $upstream = Start-Process -FilePath $node -ArgumentList @("`"$mock`"", 'openai', 'ui-fixture-key', "`"$fixture\upstream.ndjson`"", "`"$ready`"", '20') -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while (-not (Test-Path -LiteralPath $ready)) {
        if ($upstream.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Local upstream did not start.' }
        Start-Sleep -Milliseconds 100
    }
    Invoke-Installer
    $upstreamUrl = 'http://127.0.0.1:' + (Get-Content -LiteralPath $ready -Raw)
    & $cli --no-tui --json-errors provider add --preset custom --id install-fixture --base-url $upstreamUrl --protocol open_ai_chat --key ui-fixture-key --model shared | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Unable to configure the isolated provider.' }
    & $cli --no-tui --json-errors service autostart enable --json | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Unable to enable isolated gateway autostart.' }
    Assert-Gateway 'running'

    # Emulate an older task definition; the upgrade must repair it and restart the gateway.
    $oldSettings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
    Set-ScheduledTask -TaskName $taskName -TaskPath '\' -Settings $oldSettings | Out-Null
    Invoke-Installer
    Assert-Gateway 'running'
    $task = Get-ScheduledTask -TaskName $taskName -TaskPath '\'
    if (-not $task.Settings.StartWhenAvailable -or $task.Settings.RestartCount -lt 3 -or $task.Settings.RestartInterval -ne 'PT1M') {
        throw 'Upgrade did not repair the gateway startup retry settings.'
    }

    # Exercise the registered startup action again without requiring a real reboot.
    & $cli --no-tui --json-errors service stop --managed --allow-codex-disconnect --json | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Unable to stop the isolated gateway.' }
    # A deliberate stop must not be treated as a failed task and restarted.
    for ($second = 0; $second -lt 65; $second += 5) { Start-Sleep -Seconds 5 }
    Assert-Gateway 'stopped'
    Start-ScheduledTask -TaskName $taskName -TaskPath '\'
    Assert-Gateway 'running'

    & $cli --no-tui --json-errors service autostart disable --json | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Unable to disable isolated gateway autostart.' }
    Assert-Gateway 'running'
    Invoke-Installer
    Assert-Gateway 'running'
    $autostart = & $cli --no-tui --json-errors service autostart status --json | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $autostart.enabled) { throw 'Upgrade enabled an explicitly disabled autostart.' }
    & $cli --no-tui --json-errors service stop --managed --allow-codex-disconnect --json | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Unable to stop the gateway before the stopped-state upgrade.' }
    Invoke-Installer
    Assert-Gateway 'stopped'
    $autostart = & $cli --no-tui --json-errors service autostart status --json | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $autostart.enabled) { throw 'Upgrade changed the disabled autostart choice.' }
    Write-Output 'PASS: installer restores a running gateway, repairs autostart and preserves disabled startup.'
} finally {
    $task = Get-ScheduledTask -TaskName $taskName -TaskPath '\' -ErrorAction SilentlyContinue
    if ($task) {
        if ($task.State -eq 'Running') { Stop-ScheduledTask -TaskName $taskName -TaskPath '\' }
        Unregister-ScheduledTask -TaskName $taskName -TaskPath '\' -Confirm:$false
    }
    if ($upstream -and -not $upstream.HasExited) { Stop-Process -Id $upstream.Id -Force }
    $uninstaller = Join-Path $directory 'uninstall.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        $process = Start-Process -FilePath $uninstaller -ArgumentList @('/S', "_?=$directory") -Wait -PassThru -WindowStyle Hidden
        if ($process.ExitCode -ne 0) { throw "Installer cleanup failed (exit $($process.ExitCode))." }
    }
    $runKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Software\Microsoft\Windows\CurrentVersion\Run')
    try {
        if ($null -ne $previousStartup) { $runKey.SetValue('CodexMixin', $previousStartup) }
        else { $runKey.DeleteValue('CodexMixin', $false) }
    } finally { $runKey.Dispose() }
    foreach ($entry in $previousEnvironment.GetEnumerator()) { [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value) }
    if (Test-Path -LiteralPath $fixture) {
        $resolvedFixture = (Resolve-Path -LiteralPath $fixture).Path
        if (-not $resolvedFixture.StartsWith($runnerRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing to remove a fixture outside RUNNER_TEMP.' }
        Remove-Item -LiteralPath $resolvedFixture -Recurse -Force
    }
}
