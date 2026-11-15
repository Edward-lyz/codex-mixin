param(
    [Parameter(Mandatory)][string]$AppDirectory,
    [Parameter(Mandatory)][string]$WinApp,
    [Parameter(Mandatory)][string]$EvidenceDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$AppDirectory = (Resolve-Path $AppDirectory).Path
$WinApp = (Resolve-Path $WinApp).Path
$evidence = [IO.Directory]::CreateDirectory($EvidenceDirectory).FullName
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('codex-mixin-ui-' + [guid]::NewGuid().ToString('N'))
$null = [IO.Directory]::CreateDirectory($fixture)
# The in-process Win32 file dialogs resolve Desktop from USERPROFILE and refuse to open without it.
$null = [IO.Directory]::CreateDirectory("$fixture\home\Desktop")
$app = $null
$upstream = $null
$settingsChanged = $false
$startupValue = [Microsoft.Win32.Registry]::GetValue('HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run', 'CodexMixin', $null)
$automaticValue = [Microsoft.Win32.Registry]::GetValue('HKEY_CURRENT_USER\Software\CodexMixin\Updates', 'Automatic', $null)
$stage = 'startup'
$completed = [Collections.Generic.List[string]]::new()
$cli = Join-Path $AppDirectory 'codex-mixin.exe'
$originalEnvironment = @{}
$isolatedEnvironment = @{
    HOME = "$fixture\home"; USERPROFILE = "$fixture\home"
    CODEX_HOME = "$fixture\codex"; DSH_HOME = "$fixture\home\.dsh"
    CLAUDE_CONFIG_DIR = "$fixture\home\.claude"
    CODEX_GATEWAY_CONFIG = "$fixture\config.json"
    CODEX_GATEWAY_RUNTIME_FILE = "$fixture\runtime.json"
    CODEX_GATEWAY_DAEMON_FILE = "$fixture\daemon.json"
    CODEX_GATEWAY_SERVICE_ID = ('ui-' + [guid]::NewGuid().ToString('N'))
    XDG_CONFIG_HOME = "$fixture\xdg-config"; XDG_DATA_HOME = "$fixture\xdg-data"
    XDG_STATE_HOME = "$fixture\xdg-state"; XDG_CACHE_HOME = "$fixture\xdg-cache"
    APPDATA = "$fixture\appdata"; LOCALAPPDATA = "$fixture\localappdata"
    HTTP_PROXY = 'http://127.0.0.1:9'; HTTPS_PROXY = 'http://127.0.0.1:9'
    NO_PROXY = '127.0.0.1,localhost'
    WINAPP_UI_WORKFLOW_ID = [guid]::NewGuid().ToString('N')
    WINAPP_CLI_TELEMETRY_OPTOUT = '1'
    CODEX_MIXIN_LANG = 'zh'
}
foreach ($entry in $isolatedEnvironment.GetEnumerator()) {
    $originalEnvironment[$entry.Key] = [Environment]::GetEnvironmentVariable($entry.Key)
    [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
}
$null = [IO.Directory]::CreateDirectory($env:HOME)
$null = [IO.Directory]::CreateDirectory($env:CODEX_HOME)

function Wait-UiIdle {
    # RunActionAsync disables refresh while it runs and the page reloads; idle means both finished.
    & $WinApp ui wait-for shell-refresh -a $app.Id --property IsEnabled --value True --timeout 90000
}
function Open-Provider {
    & $WinApp ui click provider-card-ui-local -a $app.Id
    & $WinApp ui wait-for provider-models-save -a $app.Id --timeout 15000
}
function Select-Option([string]$Select, [string]$Text) {
    & $WinApp ui click $Select -a $app.Id
    $option = & $WinApp ui search $Text -a $app.Id --json | ConvertFrom-Json
    if (@($option.matches).Count -eq 0) { throw "Option '$Text' was not offered by $Select" }
    # Arc popups draw options themselves; click the last match, the one inside the open popup.
    & $WinApp ui click $option.matches[-1].selector -a $app.Id
}

function Invoke-AppExit {
    & $WinApp ui invoke shell-menu -a $app.Id
    & $WinApp ui invoke shell-quit -a $app.Id
}

try {
    $stage = 'local upstream'
    # A disposable loopback fixture, never a paid provider or a user credential.
    $readyFile = Join-Path $fixture 'upstream.port'
    $upstream = Start-Process node -ArgumentList @(
        'scripts/e2e_mock_provider.mjs', 'openai', 'ui-fixture-key',
        "$fixture/upstream.ndjson", $readyFile, '20'
    ) -PassThru -RedirectStandardOutput "$fixture/upstream.log" -RedirectStandardError "$fixture/upstream-error.log"
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while (-not (Test-Path $readyFile)) {
        if ($upstream.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Local upstream did not become ready' }
        Start-Sleep -Milliseconds 100
    }
    $upstreamUrl = 'http://127.0.0.1:' + (Get-Content $readyFile -Raw)
    $stage = 'startup'
    & $cli --version | Set-Content "$evidence\cli-version.txt"
    $interface = & $cli --no-tui --json-errors interface --json | ConvertFrom-Json
    if ($interface.protocol_version -ne 1) { throw 'Unexpected CLI interface version' }
    $guiVersion = [Reflection.AssemblyName]::GetAssemblyName((Join-Path $AppDirectory 'codex_mixin_ui.dll')).Version.ToString(3)
    if ($guiVersion -ne ($interface.version -split '-', 2)[0]) { throw 'Published GUI and CLI versions do not match' }
    @{ gui = $guiVersion; cli = $interface.version } | ConvertTo-Json | Set-Content "$evidence\versions.json"
    $appStartedAt = [DateTime]::Now
    $app = Start-Process (Join-Path $AppDirectory 'codex_mixin_ui.exe') -PassThru
    & $WinApp ui wait-for provider-add-card -a $app.Id --timeout 90000
    & $WinApp ui wait-for 刷新 -a $app.Id --property IsEnabled --value True --timeout 90000
    & $WinApp ui send-keys escape -a $app.Id --via send-input
    & $WinApp ui inspect -a $app.Id --depth 10 --json | Set-Content "$evidence\initial-tree.json"
    & $WinApp ui screenshot -a $app.Id --output "$evidence\initial.png"
    # With no provider the Arc add card starts expanded.
    & $WinApp ui wait-for provider-new-preset -a $app.Id --timeout 10000
    & $WinApp ui inspect -a $app.Id --depth 10 --json | Set-Content "$evidence\add-provider-tree.json"
    & $WinApp ui screenshot -a $app.Id --output "$evidence\add-provider.png"
    $completed.Add('launch, real CLI connection, interactive desktop and provider form')

    $stage = 'provider cancel and save'
    # Abandon a half-filled form through the shell's discard confirmation.
    & $WinApp ui set-value provider-new-id cancelled-provider -a $app.Id
    & $WinApp ui invoke 刷新 -a $app.Id
    & $WinApp ui invoke 取消 -a $app.Id
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    if (@($listed.providers).Count -ne 0) { throw 'Cancel created a provider' }
    Select-Option provider-new-preset 'Custom'
    & $WinApp ui set-value provider-new-id ui-local -a $app.Id
    & $WinApp ui set-value provider-new-name 'UI local provider' -a $app.Id
    & $WinApp ui set-value provider-new-base-url $upstreamUrl -a $app.Id
    Select-Option provider-new-protocol 'Chat Completions'
    & $WinApp ui set-value provider-new-key ui-fixture-key -a $app.Id
    & $WinApp ui invoke provider-new-save -a $app.Id
    Wait-UiIdle
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    $provider = @($listed.providers | Where-Object id -eq 'ui-local')[0]
    if ($null -eq $provider -or $provider.display_name -ne 'UI local provider' -or $provider.base_url -ne $upstreamUrl) { throw 'UI provider save differs from CLI state' }
    if ($provider.protocol -ne 'open_ai_chat' -or $provider.api_path -ne '/v1/chat/completions') { throw 'Explicit Chat protocol did not select its default API endpoint' }
    $completed.Add($stage)

    $stage = 'model selection, search, context and apply'
    Open-Provider
    & $WinApp ui wait-for model-table -a $app.Id --timeout 15000
    foreach ($model in @($provider.selected_models | Where-Object { $_ -ne 'shared' })) {
        & $WinApp ui invoke "model-check-$model" -a $app.Id --action toggle
    }
    if ('shared' -notin $provider.selected_models) { & $WinApp ui invoke model-check-shared -a $app.Id --action toggle }
    & $WinApp ui set-value provider-model-search shared -a $app.Id
    & $WinApp ui wait-for provider-model-search -a $app.Id --value shared --timeout 5000
    & $WinApp ui set-value model-context-shared 16384 -a $app.Id
    & $WinApp ui invoke provider-models-save -a $app.Id
    Wait-UiIdle
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    $provider = @($listed.providers | Where-Object id -eq 'ui-local')[0]
    if (@($provider.selected_models).Count -ne 1 -or $provider.selected_models[0] -ne 'shared' -or $provider.model_context_overrides.shared -ne 16384) { throw 'Model draft was not persisted by the UI' }
    $completed.Add($stage)

    $stage = 'provider edit and refresh'
    Open-Provider
    & $WinApp ui click provider-connection-ui-local -a $app.Id
    & $WinApp ui wait-for provider-edit-base-url -a $app.Id --timeout 10000
    & $WinApp ui set-value provider-edit-name 'UI local edited' -a $app.Id
    & $WinApp ui invoke provider-edit-save -a $app.Id
    Wait-UiIdle
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    if (@($listed.providers | Where-Object id -eq 'ui-local')[0].display_name -ne 'UI local edited') { throw 'Provider edit was not saved' }
    & $WinApp ui invoke 刷新 -a $app.Id
    Wait-UiIdle
    Open-Provider
    & $WinApp ui screenshot -a $app.Id --output "$evidence\models.png"
    $completed.Add($stage)

    $stage = 'real CLI failure and recovery'
    $configBytes = [IO.File]::ReadAllBytes($env:CODEX_GATEWAY_CONFIG)
    try {
        [IO.File]::WriteAllText($env:CODEX_GATEWAY_CONFIG, '{')
        & $WinApp ui invoke 刷新 -a $app.Id
        # The shell reports the CLI failure in its status line; the page must not claim success.
        & $WinApp ui wait-for operation-status -a $app.Id --timeout 90000
        & $WinApp ui screenshot -a $app.Id --output "$evidence\cli-error.png"
    }
    finally { [IO.File]::WriteAllBytes($env:CODEX_GATEWAY_CONFIG, $configBytes) }
    & $WinApp ui invoke 刷新 -a $app.Id
    Wait-UiIdle
    & $WinApp ui wait-for provider-card-ui-local -a $app.Id --timeout 30000
    $completed.Add($stage)

    $stage = 'model discovery, capability probe and benchmark'
    Open-Provider
    & $WinApp ui invoke provider-models-discover -a $app.Id
    Wait-UiIdle
    Open-Provider
    & $WinApp ui invoke provider-models-probe -a $app.Id
    Wait-UiIdle
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    $provider = @($listed.providers | Where-Object id -eq 'ui-local')[0]
    if (@($provider.selected_models).Count -ne 1 -or $provider.selected_models[0] -ne 'shared') { throw 'Discovery or probe changed the selected models' }
    & $cli --no-tui --json-errors service start
    Open-Provider
    & $WinApp ui invoke provider-models-benchmark -a $app.Id
    Wait-UiIdle
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    do {
        $benchmark = & $cli --no-tui --json-errors benchmark status | ConvertFrom-Json
        if ([DateTime]::UtcNow -gt $deadline) { throw 'UI benchmark did not complete' }
        if ($benchmark.snapshot.status -ne 'completed') { Start-Sleep -Milliseconds 200 }
    } until ($benchmark.snapshot.status -eq 'completed')
    $benchmark | ConvertTo-Json -Depth 12 | Set-Content "$evidence\benchmark.json"
    $results = @($benchmark.snapshot.results | Where-Object { $_.provider_id -eq 'ui-local' -and $_.upstream_model -eq 'shared' })
    if ($results.Count -ne 1 -or $results[0].status -ne 'completed') { throw "UI benchmark did not successfully call the loopback model: $($benchmark | ConvertTo-Json -Depth 12 -Compress)" }
    if (-not (Test-Path "$fixture/upstream.ndjson") -or (Get-Item "$fixture/upstream.ndjson").Length -eq 0) { throw 'No actual upstream requests were recorded' }
    $completed.Add($stage)

    $stage = 'Codex mode, cancel, connect and disconnect'
    & $WinApp ui send-keys ctrl+2 -a $app.Id --via send-input
    & $WinApp ui wait-for agent-summary -a $app.Id --timeout 30000
    & $WinApp ui click agent-undetected -a $app.Id
    & $WinApp ui click agent-card-codex -a $app.Id
    & $WinApp ui wait-for agent-codex-mode -a $app.Id --timeout 10000
    Select-Option agent-codex-mode '仅自定义模型'
    $agents = & $cli --no-tui --json-errors agent list --json | ConvertFrom-Json
    if (@($agents.agents | Where-Object id -eq 'codex')[0].integration.state -ne 'unmanaged') { throw 'Choosing a Codex mode mutated CLI state' }
    & $WinApp ui invoke agent-switch-codex -a $app.Id --action toggle
    Wait-UiIdle
    $agents = & $cli --no-tui --json-errors agent list --json | ConvertFrom-Json
    $codex = @($agents.agents | Where-Object id -eq 'codex')[0]
    if ($codex.integration.state -ne 'managed' -or $codex.integration.mode -ne 'custom_only') { throw 'Codex switch failed to save the selected mode' }
    & $WinApp ui screenshot -a $app.Id --output "$evidence\agents.png"
    & $WinApp ui click agent-card-codex -a $app.Id
    & $WinApp ui invoke agent-switch-codex -a $app.Id --action toggle
    & $WinApp ui invoke 取消 -a $app.Id
    $agents = & $cli --no-tui --json-errors agent list --json | ConvertFrom-Json
    if (@($agents.agents | Where-Object id -eq 'codex')[0].integration.state -ne 'managed') { throw 'Cancelled disconnect removed Codex integration' }
    & $WinApp ui invoke agent-switch-codex -a $app.Id --action toggle
    & $WinApp ui invoke 确认 -a $app.Id
    Wait-UiIdle
    $agents = & $cli --no-tui --json-errors agent list --json | ConvertFrom-Json
    if (@($agents.agents | Where-Object id -eq 'codex')[0].integration.state -ne 'unmanaged') { throw 'Codex disconnect did not restore unmanaged state' }
    $completed.Add($stage)

    $stage = 'context toggle and save'
    # The console is one window; the shell shortcut Ctrl+4 opens the context page.
    & $WinApp ui send-keys ctrl+4 -a $app.Id --via send-input
    & $WinApp ui wait-for context-enabled -a $app.Id --timeout 90000
    Wait-UiIdle
    $before = & $cli --no-tui --json-errors context config get --json | ConvertFrom-Json
    & $WinApp ui invoke context-enabled -a $app.Id --action toggle
    & $WinApp ui invoke context-save -a $app.Id
    Wait-UiIdle
    $after = & $cli --no-tui --json-errors context config get --json | ConvertFrom-Json
    if ($before.context.enabled -eq $after.context.enabled) { throw 'Context switch did not change saved CLI state' }
    & $WinApp ui wait-for context-enabled -a $app.Id --timeout 30000
    & $WinApp ui invoke context-enabled -a $app.Id --action toggle
    & $WinApp ui invoke context-save -a $app.Id
    Wait-UiIdle
    $restored = & $cli --no-tui --json-errors context config get --json | ConvertFrom-Json
    if ($before.context.enabled -ne $restored.context.enabled) { throw 'Context switch did not restore its original value' }
    $completed.Add($stage)

    $stage = 'usage filters'
    & $WinApp ui send-keys ctrl+5 -a $app.Id --via send-input
    & $WinApp ui wait-for usage-range -a $app.Id --timeout 30000
    # The range segments reload the page data in place; the trend title names the loaded range.
    & $WinApp ui click 今天 -a $app.Id
    & $WinApp ui wait-for usage-trend-title -a $app.Id --value '按小时' --contains --timeout 30000
    & $WinApp ui click '按 Provider 拆分' -a $app.Id
    & $WinApp ui wait-for usage-status -a $app.Id --timeout 10000
    $null = & $cli --no-tui --json-errors usage --daily --json --days 1 | ConvertFrom-Json
    & $WinApp ui screenshot -a $app.Id --output "$evidence\usage.png"
    $completed.Add($stage)

    $stage = 'model routing: fusion, schedule, failover'
    # Failover needs a second routable model besides `shared`.
    $null = & $cli --no-tui --json-errors provider select ui-local --model shared --model hidden
    if ($LASTEXITCODE -ne 0) { throw 'Selecting a second model for routing failed' }
    & $WinApp ui send-keys ctrl+3 -a $app.Id --via send-input
    & $WinApp ui wait-for route-new-fusion -a $app.Id --timeout 90000
    & $WinApp ui invoke route-new-fusion -a $app.Id
    & $WinApp ui set-value route-name ui-fusion -a $app.Id
    & $WinApp ui invoke route-save -a $app.Id
    Wait-UiIdle
    $routes = & $cli --no-tui --json-errors route list --json | ConvertFrom-Json
    $fusion = @($routes.routes | Where-Object model -eq 'auto/ui-fusion')[0]
    if ($null -eq $fusion -or $fusion.kind -ne 'fusion' -or $fusion.mode -ne 'orchestration') { throw 'Fusion route was not saved' }
    & $WinApp ui invoke route-new-schedule -a $app.Id
    & $WinApp ui invoke route-slot-add -a $app.Id
    & $WinApp ui wait-for route-slot-remove-1 -a $app.Id --timeout 10000
    & $WinApp ui invoke route-slot-remove-0 -a $app.Id
    & $WinApp ui invoke route-save -a $app.Id
    Wait-UiIdle
    $routes = & $cli --no-tui --json-errors route list --json | ConvertFrom-Json
    $schedule = @($routes.routes | Where-Object kind -eq 'schedule')[0]
    if ($null -eq $schedule -or $schedule.mode -ne 'time_rotation' -or @($schedule.time_routes).Count -ne 1) { throw 'Schedule route was not saved' }
    & $WinApp ui invoke route-new-failover -a $app.Id
    & $WinApp ui invoke route-save -a $app.Id
    Wait-UiIdle
    $routes = & $cli --no-tui --json-errors route list --json | ConvertFrom-Json
    $failover = @($routes.routes | Where-Object kind -eq 'failover')[0]
    if ($null -eq $failover -or @($failover.fallbacks).Count -ne 1 -or $failover.fallbacks[0] -eq $failover.source) { throw 'Failover route was not saved' }
    & $WinApp ui screenshot -a $app.Id --output "$evidence\routing.png"
    # The saved failover route stays selected; cancel, then confirm its deletion.
    & $WinApp ui invoke route-delete -a $app.Id
    & $WinApp ui invoke 取消 -a $app.Id
    $routes = & $cli --no-tui --json-errors route list --json | ConvertFrom-Json
    if (@($routes.routes).Count -ne 3) { throw 'Cancelled route deletion removed a route' }
    & $WinApp ui invoke route-delete -a $app.Id
    & $WinApp ui invoke 确认 -a $app.Id
    Wait-UiIdle
    $routes = & $cli --no-tui --json-errors route list --json | ConvertFrom-Json
    if (@($routes.routes | Where-Object kind -eq 'failover').Count -ne 0 -or @($routes.routes).Count -ne 2) { throw 'Route deletion did not remove the failover route' }
    # Later stages delete the provider, which routes still reference.
    foreach ($route in @($routes.routes)) {
        $null = & $cli --no-tui --json-errors route delete --model $route.model
        if ($LASTEXITCODE -ne 0) { throw "Removing route $($route.model) failed" }
    }
    $completed.Add($stage)

    $stage = 'settings diagnostics, updates and backup cancellation'
    & $WinApp ui send-keys ctrl+6 -a $app.Id --via send-input
    & $WinApp ui wait-for run-diagnostics -a $app.Id --timeout 90000
    Wait-UiIdle
    $settingsChanged = $true
    $automatic = 'app-update-automatic'
    & $WinApp ui invoke $automatic -a $app.Id --action toggle
    $automaticAfter = [Microsoft.Win32.Registry]::GetValue('HKEY_CURRENT_USER\Software\CodexMixin\Updates', 'Automatic', $null)
    $automaticBefore = if ($null -eq $automaticValue) { 1 } else { [int]$automaticValue }
    if ($null -eq $automaticAfter -or [int]$automaticAfter -eq $automaticBefore) { throw 'Automatic-update switch did not save the new state' }
    & $WinApp ui invoke $automatic -a $app.Id --action toggle
    if ([int][Microsoft.Win32.Registry]::GetValue('HKEY_CURRENT_USER\Software\CodexMixin\Updates', 'Automatic', -1) -ne $automaticBefore) { throw 'Automatic-update switch did not restore its enabled state' }
    & $WinApp ui invoke run-diagnostics -a $app.Id
    & $WinApp ui wait-for doctor-report -a $app.Id --timeout 180000
    & $WinApp ui screenshot -a $app.Id --output "$evidence\doctor.png"
    & $WinApp ui send-keys escape -a $app.Id --via send-input
    $null = & $cli --no-tui --json-errors doctor --json | ConvertFrom-Json
    # Normal HTTP uses the dead fixture proxy; an update check must surface its error.
    & $WinApp ui wait-for check-app-updates -a $app.Id --property IsEnabled --value True --timeout 90000
    & $WinApp ui invoke check-app-updates -a $app.Id
    & $WinApp ui wait-for app-update-status -a $app.Id --value '更新失败：' --contains --timeout 90000
    & $WinApp ui wait-for check-app-updates -a $app.Id --property IsEnabled --value True --timeout 90000
    $completed.Add($stage)

    $stage = 'ECH test, safe fallback and disable'
    & $WinApp ui wait-for ech-test -a $app.Id --timeout 30000
    $echBefore = & $cli --no-tui --json-errors ech status --json | ConvertFrom-Json
    & $WinApp ui invoke ech-test -a $app.Id
    # ECH deliberately bypasses ordinary proxies. Both real probe verdicts are valid:
    # a ready probe lists its hosts, a failed one reports the CLI error.
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    $echTestStatus = ''
    do {
        foreach ($outcome in @('ech-test-result', 'settings-error')) {
            $found = & $WinApp ui search $outcome -a $app.Id --json | ConvertFrom-Json
            if (@($found.matches).Count -gt 0) { $echTestStatus = $outcome; break }
        }
        if ([DateTime]::UtcNow -gt $deadline) { throw 'ECH test did not expose its terminal result' }
        if ($echTestStatus -eq '') { Start-Sleep -Milliseconds 200 }
    } until ($echTestStatus -ne '')
    & $WinApp ui send-keys escape -a $app.Id --via send-input
    Start-Sleep -Milliseconds 500
    $echAfter = & $cli --no-tui --json-errors ech status --json | ConvertFrom-Json
    if ($echAfter.enabled -ne $echBefore.enabled -or $echAfter.fallback_reason -ne $echBefore.fallback_reason) { throw 'ECH test changed saved network settings' }
    & $WinApp ui invoke ech-enabled -a $app.Id --action toggle
    Wait-UiIdle
    $echAfter = & $cli --no-tui --json-errors ech status --json | ConvertFrom-Json
    if ($echAfter.enabled -eq (-not [string]::IsNullOrWhiteSpace($echAfter.fallback_reason))) { throw 'ECH enable did not persist a consistent probe verdict or safe fallback' }
    @{ test = $echTestStatus; enable = $echAfter } | ConvertTo-Json -Depth 8 | Set-Content "$evidence\ech.json"
    & $WinApp ui wait-for ech-enabled -a $app.Id --timeout 90000
    & $WinApp ui invoke ech-enabled -a $app.Id --action toggle
    Wait-UiIdle
    $echAfter = & $cli --no-tui --json-errors ech status --json | ConvertFrom-Json
    if ($echAfter.enabled -or $null -ne $echAfter.fallback_reason) { throw 'ECH disable did not clear its saved setting and fallback reason' }
    $completed.Add($stage)

    & $WinApp ui invoke export-config -a $app.Id
    & $WinApp ui wait-for 取消 -a $app.Id --timeout 5000
    & $WinApp ui invoke 取消 -a $app.Id
    if (-not (Test-Path $env:CODEX_GATEWAY_CONFIG)) { throw 'Backup cancellation removed configuration' }
    $stage = 'native backup export, import and restoration'
    $backup = Join-Path $fixture 'config.b64'
    $beforeBackup = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    & $WinApp ui invoke export-config -a $app.Id
    & $WinApp ui invoke 确认 -a $app.Id
    & $WinApp ui wait-for 'File name:' -a $app.Id --timeout 30000
    $pickerTree = & $WinApp ui inspect -a $app.Id --depth 10 --json | ConvertFrom-Json
    $pickerTree | ConvertTo-Json -Depth 30 | Set-Content "$evidence\export-picker-tree.json"
    # The picker title is the one the page passes to the storage provider.
    $pickerWindow = @($pickerTree.windows | Where-Object title -eq '选择保存位置')[0].hwnd
    # Target the native dialog, not the larger disabled window behind it.
    $field = & $WinApp ui search 'File name:' -w $pickerWindow --type Edit --json | ConvertFrom-Json
    & $WinApp ui send-keys 'ctrl+a' --target $field.matches[0].selector -w $pickerWindow --via send-input
    & $WinApp ui send-keys $backup --verbatim --target $field.matches[0].selector -w $pickerWindow --via send-input
    & $WinApp ui wait-for $field.matches[0].selector -w $pickerWindow --value $backup --timeout 5000
    & $WinApp ui send-keys tab -w $pickerWindow --via send-input
    $button = & $WinApp ui search Save -w $pickerWindow --type Button --json | ConvertFrom-Json
    & $WinApp ui invoke $button.matches[0].selector -w $pickerWindow
    Wait-UiIdle
    if (-not (Test-Path $backup) -or (Get-Item $backup).Length -eq 0) { throw 'Native save picker did not export the fixture' }
    & $cli --no-tui --json-errors provider update ui-local --display-name 'Changed after backup'
    & $WinApp ui invoke import-config -a $app.Id
    & $WinApp ui wait-for 'File name:' -a $app.Id --timeout 30000
    $pickerTree = & $WinApp ui inspect -a $app.Id --depth 10 --json | ConvertFrom-Json
    $pickerWindow = @($pickerTree.windows | Where-Object title -eq '导入配置')[0].hwnd
    $field = & $WinApp ui search 'File name:' -w $pickerWindow --type Edit --json | ConvertFrom-Json
    & $WinApp ui send-keys 'ctrl+a' --target $field.matches[0].selector -w $pickerWindow --via send-input
    & $WinApp ui send-keys $backup --verbatim --target $field.matches[0].selector -w $pickerWindow --via send-input
    & $WinApp ui wait-for $field.matches[0].selector -w $pickerWindow --value $backup --timeout 5000
    & $WinApp ui send-keys tab -w $pickerWindow --via send-input
    $button = & $WinApp ui search Open -w $pickerWindow --type Button --json | ConvertFrom-Json
    # The Open split button also exposes its dropdown arrow as a Button; invoke the main part.
    $openButton = @($button.matches | Where-Object { $_.selector -notlike '*dropdown*' })[0]
    & $WinApp ui invoke $openButton.selector -w $pickerWindow
    & $WinApp ui wait-for 确认 -a $app.Id --timeout 15000
    & $WinApp ui invoke 确认 -a $app.Id
    Wait-UiIdle
    $afterBackup = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    if ($afterBackup.providers[0].display_name -ne $beforeBackup.providers[0].display_name) { throw 'Native import did not restore the exported provider' }
    Remove-Item $backup
    $completed.Add($stage)
    & $WinApp ui inspect -a $app.Id --depth 10 --json | Set-Content "$evidence\settings-tree.json"

    $stage = 'window close to tray, reopen, refresh and provider delete'
    # With the tray icon present, closing the console hides it and keeps the process and gateway alive.
    & $WinApp ui send-keys alt+f4 -a $app.Id --via send-input --allow-system-keys
    Start-Sleep -Seconds 2
    if ($app.HasExited) { throw 'Closing the console window exited the application instead of hiding it' }
    $tree = & $WinApp ui inspect -a $app.Id --json | ConvertFrom-Json
    if (@($tree.windows | Where-Object title -eq 'Codex Mixin').Count -ne 0) { throw 'Closing the console window left it visible' }
    $before = & $cli --no-tui --json-errors service status --json | ConvertFrom-Json
    if ($before.gateway -ne 'running') { throw 'Hiding the console stopped the gateway' }
    # A second launch hands its page request to the running instance.
    $null = Start-Process (Join-Path $AppDirectory 'codex_mixin_ui.exe') -ArgumentList '--page=providers' -PassThru
    & $WinApp ui wait-for shell-refresh -a $app.Id --timeout 30000
    & $WinApp ui invoke shell-refresh -a $app.Id
    Wait-UiIdle
    & $WinApp ui screenshot -a $app.Id --output "$evidence\reopened.png"
    & $WinApp ui wait-for provider-card-ui-local -a $app.Id --timeout 30000
    Open-Provider
    & $WinApp ui invoke provider-remove-ui-local -a $app.Id
    & $WinApp ui invoke 取消 -a $app.Id
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    if (@($listed.providers | Where-Object id -eq 'ui-local').Count -ne 1) { throw 'Cancelled provider deletion removed configuration' }
    & $WinApp ui invoke provider-remove-ui-local -a $app.Id
    & $WinApp ui invoke 确认 -a $app.Id
    Wait-UiIdle
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    if (@($listed.providers).Count -ne 0) { throw 'Provider delete button did not remove the isolated provider' }
    $completed.Add('provider delete cancellation and confirmation')
    $completed.Add($stage)

    $stage = 'exit with unsaved changes'
    # The empty provider page opens the add form; typing into it leaves an unsaved draft.
    & $WinApp ui wait-for provider-new-id -a $app.Id --timeout 30000
    & $WinApp ui set-value provider-new-id unsaved-draft -a $app.Id
    Invoke-AppExit
    & $WinApp ui invoke 取消 -a $app.Id
    if ($app.WaitForExit(3000)) { throw 'Cancel on the unsaved-changes confirmation exited the application' }
    Invoke-AppExit
    & $WinApp ui invoke 确认 -a $app.Id
    if (-not $app.WaitForExit(30000)) { throw 'Exit action did not exit the application' }
    $completed.Add($stage)
    @{ passed = $completed.ToArray(); phase = 'native UI and CLI'; notCovered = @('provider OAuth and credential-dependent adapters', 'installing an application update (covered separately by installer and portable-update checks)') } | ConvertTo-Json -Depth 6 | Set-Content "$evidence\coverage.json"
}
catch {
    @{ stage = $stage; passed = $completed.ToArray(); error = $_.Exception.Message } |
        ConvertTo-Json -Depth 6 | Set-Content "$evidence\coverage.json"
    throw
}
finally {
    # Preserve evidence even when an action fails; cleanup cannot turn failure into success.
    $PSNativeCommandUseErrorActionPreference = $false
    if ($null -ne $app) {
        & $WinApp ui inspect -a $app.Id --depth 10 --json 2> "$evidence\inspect-error.txt" | Set-Content "$evidence\final-tree.json"
        & $WinApp ui screenshot -a $app.Id --output "$evidence\final.png" 2> "$evidence\screenshot-error.txt"
        if ($app.HasExited) {
            # UI Automation reports only a stale element when the process dies.
            # Preserve the native crash cause instead of losing it with the runner.
            @{ pid = $app.Id; exitCode = $app.ExitCode; stage = $stage } |
                ConvertTo-Json | Set-Content "$evidence\app-exit.json"
            @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = $appStartedAt; Level = 2 } -ErrorAction SilentlyContinue |
                Where-Object { $_.Message -match 'codex_mixin_ui|CodexMixin|PresentationFramework' } |
                Select-Object TimeCreated, ProviderName, Id, Message) |
                ConvertTo-Json -Depth 4 | Set-Content "$evidence\app-crashes.json"
        }
        else { Stop-Process -Id $app.Id -Force -ErrorAction Continue }
    }
    & $cli --no-tui --json-errors service stop --force --allow-codex-disconnect --restore-clients *> "$evidence\service-cleanup.txt"
    if ($null -ne $upstream -and -not $upstream.HasExited) { Stop-Process -Id $upstream.Id -Force -ErrorAction Continue }
    if ($settingsChanged) {
        $runKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Software\Microsoft\Windows\CurrentVersion\Run')
        if ($null -eq $startupValue) { $runKey.DeleteValue('CodexMixin', $false) }
        else { $runKey.SetValue('CodexMixin', $startupValue, [Microsoft.Win32.RegistryValueKind]::String) }
        $runKey.Dispose()
        $updateKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Software\CodexMixin\Updates')
        if ($null -eq $automaticValue) { $updateKey.DeleteValue('Automatic', $false) }
        else { $updateKey.SetValue('Automatic', $automaticValue, [Microsoft.Win32.RegistryValueKind]::DWord) }
        $updateKey.Dispose()
    }
    $guiLogs = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'CodexMixin/logs'
    if (Test-Path $guiLogs) { Copy-Item $guiLogs "$evidence\gui-logs" -Recurse }
    & $WinApp ui yield *> $null
    foreach ($entry in $originalEnvironment.GetEnumerator()) {
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
    }
    Remove-Item $fixture -Recurse -Force
}
