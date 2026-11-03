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
}
foreach ($entry in $isolatedEnvironment.GetEnumerator()) {
    $originalEnvironment[$entry.Key] = [Environment]::GetEnvironmentVariable($entry.Key)
    [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
}
$null = [IO.Directory]::CreateDirectory($env:HOME)
$null = [IO.Directory]::CreateDirectory($env:CODEX_HOME)

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
    $app = Start-Process (Join-Path $AppDirectory 'codex_mixin_ui.exe') -PassThru
    & $WinApp ui wait-for operation-status -a $app.Id --timeout 60000
    & $WinApp ui wait-for operation-status -a $app.Id --value '连接 CLI完成' --contains --timeout 90000
    & $WinApp ui send-keys escape -a $app.Id --via send-input
    & $WinApp ui inspect -a $app.Id --depth 10 --json | Set-Content "$evidence\initial-tree.json"
    & $WinApp ui screenshot -a $app.Id --output "$evidence\initial.png"
    $add = & $WinApp ui search 新增服务商 -a $app.Id --type Button --json | ConvertFrom-Json
    & $WinApp ui invoke $add.matches[0].selector -a $app.Id
    & $WinApp ui wait-for PrimaryButton -a $app.Id --timeout 5000
    & $WinApp ui inspect -a $app.Id --depth 10 --json | Set-Content "$evidence\add-provider-tree.json"
    & $WinApp ui screenshot -a $app.Id --output "$evidence\add-provider.png"
    $completed.Add('launch, real CLI connection, interactive desktop and provider dialog')

    $stage = 'provider cancel and save'
    & $WinApp ui invoke CloseButton -a $app.Id
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    if (@($listed.providers).Count -ne 0) { throw 'Cancel created a provider' }
    $add = & $WinApp ui search 新增服务商 -a $app.Id --type Button --json | ConvertFrom-Json
    & $WinApp ui invoke $add.matches[0].selector -a $app.Id
    & $WinApp ui invoke 供应商类型 -a $app.Id --action expand
    $option = & $WinApp ui search 'custom' -a $app.Id --type ListItem --json | ConvertFrom-Json
    & $WinApp ui invoke $option.matches[0].selector -a $app.Id --action select
    $field = & $WinApp ui search '供应商 ID' -a $app.Id --type Edit --json | ConvertFrom-Json
    & $WinApp ui set-value $field.matches[0].selector ui-local -a $app.Id
    $field = & $WinApp ui search '显示名称' -a $app.Id --type Edit --json | ConvertFrom-Json
    & $WinApp ui set-value $field.matches[0].selector 'UI local provider' -a $app.Id
    & $WinApp ui set-value provider-base-url $upstreamUrl -a $app.Id
    & $WinApp ui invoke '协议（custom）' -a $app.Id --action expand
    $option = & $WinApp ui search 'open_ai_chat' -a $app.Id --type ListItem --json | ConvertFrom-Json
    & $WinApp ui invoke $option.matches[0].selector -a $app.Id --action select
    $field = & $WinApp ui search 'API Key' -a $app.Id --type Edit --json | ConvertFrom-Json
    & $WinApp ui set-value $field.matches[0].selector ui-fixture-key -a $app.Id
    & $WinApp ui invoke PrimaryButton -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '保存供应商完成' --contains --timeout 90000
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    $provider = @($listed.providers | Where-Object id -eq 'ui-local')[0]
    if ($provider.display_name -ne 'UI local provider' -or $provider.base_url -ne $upstreamUrl) { throw 'UI provider save differs from CLI state' }
    if ($provider.protocol -ne 'open_ai_chat' -or $provider.api_path -ne '/v1/chat/completions') { throw 'Explicit Chat protocol did not select its default API endpoint' }
    $completed.Add($stage)

    $stage = 'model selection, search, context and apply'
    & $WinApp ui wait-for model-table -a $app.Id --timeout 15000
    foreach ($model in @($provider.selected_models | Where-Object { $_ -ne 'shared' })) {
        & $WinApp ui invoke "$model 加入 Codex" -a $app.Id --action toggle
    }
    if ('shared' -notin $provider.selected_models) { & $WinApp ui invoke 'shared 加入 Codex' -a $app.Id --action toggle }
    $field = & $WinApp ui search '搜索当前服务商的模型' -a $app.Id --type Edit --json | ConvertFrom-Json
    & $WinApp ui set-value $field.matches[0].selector shared -a $app.Id
    & $WinApp ui wait-for $field.matches[0].selector -a $app.Id --value shared --timeout 5000
    & $WinApp ui invoke 'shared 上下文' -a $app.Id
    $field = & $WinApp ui search '上下文 Token 数' -a $app.Id --type Edit --json | ConvertFrom-Json
    & $WinApp ui set-value $field.matches[0].selector 16384 -a $app.Id
    & $WinApp ui invoke PrimaryButton -a $app.Id
    & $WinApp ui invoke apply-changes -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '应用模型更改完成' --contains --timeout 90000
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    $provider = @($listed.providers | Where-Object id -eq 'ui-local')[0]
    if (@($provider.selected_models).Count -ne 1 -or $provider.selected_models[0] -ne 'shared' -or $provider.model_context_overrides.shared -ne 16384) { throw 'Model draft was not persisted by the UI' }
    $completed.Add($stage)

    $stage = 'provider edit and refresh'
    & $WinApp ui invoke provider-connection-tab -a $app.Id --action select
    & $WinApp ui wait-for provider-base-url -a $app.Id --timeout 10000
    $field = & $WinApp ui search '显示名称' -a $app.Id --type Edit --json | ConvertFrom-Json
    & $WinApp ui set-value $field.matches[0].selector 'UI local edited' -a $app.Id
    & $WinApp ui invoke apply-changes -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '保存供应商完成' --contains --timeout 90000
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    if (@($listed.providers | Where-Object id -eq 'ui-local')[0].display_name -ne 'UI local edited') { throw 'Provider edit was not saved' }
    & $WinApp ui invoke provider-models-tab -a $app.Id --action select
    & $WinApp ui invoke 刷新 -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '刷新完成' --contains --timeout 90000
    & $WinApp ui screenshot -a $app.Id --output "$evidence\models.png"
    $completed.Add($stage)

    $stage = 'real CLI failure and recovery'
    $configBytes = [IO.File]::ReadAllBytes($env:CODEX_GATEWAY_CONFIG)
    try {
        [IO.File]::WriteAllText($env:CODEX_GATEWAY_CONFIG, '{')
        & $WinApp ui invoke 刷新 -a $app.Id
        & $WinApp ui wait-for operation-status -a $app.Id --value '刷新失败' --timeout 90000
        & $WinApp ui screenshot -a $app.Id --output "$evidence\cli-error.png"
    }
    finally { [IO.File]::WriteAllBytes($env:CODEX_GATEWAY_CONFIG, $configBytes) }
    & $WinApp ui invoke 刷新 -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '刷新完成' --contains --timeout 90000
    $completed.Add($stage)

    $stage = 'model discovery, capability probe and benchmark'
    & $WinApp ui invoke 更多模型操作 -a $app.Id
    & $WinApp ui invoke 刷新模型列表 -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '刷新模型完成' --contains --timeout 90000
    & $WinApp ui invoke 更多模型操作 -a $app.Id
    & $WinApp ui invoke 重新探测已加入模型的能力 -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '探测模型能力完成' --contains --timeout 90000
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    $provider = @($listed.providers | Where-Object id -eq 'ui-local')[0]
    if (@($provider.selected_models).Count -ne 1 -or $provider.selected_models[0] -ne 'shared') { throw 'Discovery or probe changed the selected models' }
    & $cli --no-tui --json-errors service start
    & $WinApp ui invoke 更多模型操作 -a $app.Id
    & $WinApp ui invoke '测试已加入的 1 个模型' -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '启动模型测速完成' --contains --timeout 90000
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
    & $WinApp ui invoke console-agents-tab -a $app.Id --action select
    & $WinApp ui wait-for agent-summary -a $app.Id --timeout 30000
    & $WinApp ui invoke agent-undetected -a $app.Id --action expand
    & $WinApp ui invoke 'Codex 接入' -a $app.Id --action toggle
    & $WinApp ui wait-for PrimaryButton -a $app.Id --timeout 10000
    & $WinApp ui invoke CloseButton -a $app.Id
    $agents = & $cli --no-tui --json-errors agent list --json | ConvertFrom-Json
    if (@($agents.agents | Where-Object id -eq 'codex')[0].integration.state -ne 'unmanaged') { throw 'Cancelled Codex connection mutated CLI state' }
    & $WinApp ui invoke 'Codex 接入' -a $app.Id --action toggle
    & $WinApp ui invoke 接入模式 -a $app.Id --action expand
    $option = & $WinApp ui search '仅自定义模型' -a $app.Id --type ListItem --json | ConvertFrom-Json
    & $WinApp ui invoke $option.matches[0].selector -a $app.Id --action select
    & $WinApp ui invoke PrimaryButton -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '接入 Agent完成' --contains --timeout 90000
    $agents = & $cli --no-tui --json-errors agent list --json | ConvertFrom-Json
    $codex = @($agents.agents | Where-Object id -eq 'codex')[0]
    if ($codex.integration.state -ne 'managed' -or $codex.integration.mode -ne 'custom_only') { throw 'Codex switch failed to save the selected mode' }
    & $WinApp ui screenshot -a $app.Id --output "$evidence\agents.png"
    & $WinApp ui invoke 'Codex 接入' -a $app.Id --action toggle
    & $WinApp ui invoke CloseButton -a $app.Id
    $agents = & $cli --no-tui --json-errors agent list --json | ConvertFrom-Json
    if (@($agents.agents | Where-Object id -eq 'codex')[0].integration.state -ne 'managed') { throw 'Cancelled disconnect removed Codex integration' }
    & $WinApp ui invoke 'Codex 接入' -a $app.Id --action toggle
    & $WinApp ui invoke PrimaryButton -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '断开 Agent完成' --contains --timeout 90000
    $agents = & $cli --no-tui --json-errors agent list --json | ConvertFrom-Json
    if (@($agents.agents | Where-Object id -eq 'codex')[0].integration.state -ne 'unmanaged') { throw 'Codex disconnect did not restore unmanaged state' }
    $completed.Add($stage)

    $stage = 'context toggle and save'
    & $WinApp ui invoke console-context-tab -a $app.Id --action select
    & $WinApp ui wait-for context-enabled -a $app.Id --timeout 30000
    $before = & $cli --no-tui --json-errors context config get --json | ConvertFrom-Json
    & $WinApp ui invoke context-enabled -a $app.Id --action toggle
    & $WinApp ui invoke apply-changes -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '保存并应用设置完成' --contains --timeout 90000
    $after = & $cli --no-tui --json-errors context config get --json | ConvertFrom-Json
    if ($before.context.enabled -eq $after.context.enabled) { throw 'Context switch did not change saved CLI state' }
    & $WinApp ui invoke context-enabled -a $app.Id --action toggle
    & $WinApp ui invoke apply-changes -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '保存并应用设置完成' --contains --timeout 90000
    $restored = & $cli --no-tui --json-errors context config get --json | ConvertFrom-Json
    if ($before.context.enabled -ne $restored.context.enabled) { throw 'Context switch did not restore its original value' }
    $completed.Add($stage)

    $stage = 'usage filters'
    & $WinApp ui invoke console-usage-tab -a $app.Id --action select
    & $WinApp ui wait-for operation-status -a $app.Id --value '切换页面完成' --contains --timeout 90000
    & $WinApp ui wait-for 统计范围 -a $app.Id --timeout 30000
    & $WinApp ui invoke 统计范围 -a $app.Id --action expand
    $option = & $WinApp ui search '今天' -a $app.Id --type ListItem --json | ConvertFrom-Json
    # SelectionChanged disables the page while reloading; use a real mouse gesture
    # so the outgoing SelectionItem peer is not queried after it becomes disabled.
    & $WinApp ui click $option.matches[0].selector -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '读取用量完成' --contains --timeout 30000
    & $WinApp ui invoke 图表分组 -a $app.Id --action expand
    $option = & $WinApp ui search '按 Provider 拆分' -a $app.Id --type ListItem --json | ConvertFrom-Json
    & $WinApp ui invoke $option.matches[0].selector -a $app.Id --action select
    & $WinApp ui wait-for 每日用量 -a $app.Id --timeout 10000
    $null = & $cli --no-tui --json-errors usage --daily --json --days 1 | ConvertFrom-Json
    & $WinApp ui screenshot -a $app.Id --output "$evidence\usage.png"
    $completed.Add($stage)

    $stage = 'Fusion orchestration and time rotation'
    & $WinApp ui invoke window-menu -a $app.Id
    $option = & $WinApp ui search 'Fusion 设置' -a $app.Id --type MenuItem --json | ConvertFrom-Json
    & $WinApp ui invoke $option.matches[0].selector -a $app.Id
    & $WinApp ui wait-for fusion-mode -a $app.Id --timeout 30000
    $tree = & $WinApp ui inspect -a $app.Id --json | ConvertFrom-Json
    $fusionWindow = @($tree.windows | Where-Object title -eq 'Fusion 设置')[0].hwnd
    & $WinApp ui wait-for operation-status -w $fusionWindow --value '连接 CLI完成' --contains --timeout 90000
    $field = & $WinApp ui search 'Profile ID' -w $fusionWindow --type Edit --json | ConvertFrom-Json
    & $WinApp ui set-value $field.matches[0].selector ui-profile -w $fusionWindow
    & $WinApp ui invoke 显示中间结果 -w $fusionWindow --action toggle
    & $WinApp ui invoke apply-changes -w $fusionWindow
    & $WinApp ui wait-for operation-status -w $fusionWindow --value '保存 Fusion完成' --contains --timeout 90000
    $fusion = & $cli --no-tui --json-errors fusion get --json | ConvertFrom-Json
    if ($fusion.profile.id -ne 'ui-profile' -or $fusion.profile.mode -ne 'orchestration' -or -not $fusion.profile.show_intermediate_results) { throw 'Fusion orchestration was not saved' }
    & $WinApp ui invoke fusion-mode -w $fusionWindow --action expand
    $option = & $WinApp ui search 按时间轮转 -w $fusionWindow --type ListItem --json | ConvertFrom-Json
    & $WinApp ui invoke $option.matches[0].selector -w $fusionWindow --action select
    & $WinApp ui invoke 添加时段 -w $fusionWindow
    $routes = & $WinApp ui search 移除此时段 -w $fusionWindow --type Button --json | ConvertFrom-Json
    if ($routes.matchCount -ne 2) { throw 'Add time route did not create a second row' }
    & $WinApp ui invoke $routes.matches[0].selector -w $fusionWindow
    & $WinApp ui invoke apply-changes -w $fusionWindow
    & $WinApp ui wait-for operation-status -w $fusionWindow --value '保存 Fusion完成' --contains --timeout 90000
    $fusion = & $cli --no-tui --json-errors fusion get --json | ConvertFrom-Json
    if ($fusion.profile.mode -ne 'time_rotation' -or @($fusion.profile.time_routes).Count -ne 1) { throw 'Fusion time route was not saved' }
    & $WinApp ui screenshot -w $fusionWindow --output "$evidence\fusion.png"
    & $WinApp ui invoke '停用 Fusion' -w $fusionWindow
    & $WinApp ui invoke CloseButton -w $fusionWindow
    $fusion = & $cli --no-tui --json-errors fusion get --json | ConvertFrom-Json
    if ($null -eq $fusion.profile) { throw 'Cancelled Fusion deletion removed the profile' }
    & $WinApp ui invoke '停用 Fusion' -w $fusionWindow
    & $WinApp ui invoke PrimaryButton -w $fusionWindow
    & $WinApp ui wait-for operation-status -w $fusionWindow --value '停用 Fusion完成' --contains --timeout 90000
    $fusion = & $cli --no-tui --json-errors fusion get --json | ConvertFrom-Json
    if ($null -ne $fusion.profile) { throw 'Fusion deletion did not remove the profile' }
    & $WinApp ui send-keys alt+f4 -w $fusionWindow --via send-input --allow-system-keys
    $completed.Add($stage)

    $stage = 'settings themes, diagnostics and backup cancellation'
    $null = Start-Process (Join-Path $AppDirectory 'codex_mixin_ui.exe') -ArgumentList '--page=settings' -PassThru
    & $WinApp ui wait-for theme-picker -a $app.Id --timeout 30000
    $tree = & $WinApp ui inspect -a $app.Id --json | ConvertFrom-Json
    $settingsWindow = @($tree.windows | Where-Object title -eq '设置与诊断')[0].hwnd
    & $WinApp ui wait-for operation-status -w $settingsWindow --value '连接 CLI完成' --contains --timeout 90000
    foreach ($theme in @('浅色', '深色', '跟随系统')) {
        & $WinApp ui invoke theme-picker -w $settingsWindow --action expand
        $option = & $WinApp ui search $theme -w $settingsWindow --type ListItem --json | ConvertFrom-Json
        & $WinApp ui invoke $option.matches[0].selector -w $settingsWindow --action select
        & $WinApp ui screenshot -w $settingsWindow --output "$evidence\theme-$theme.png"
    }
    $settingsChanged = $true
    & $WinApp ui click '登录 Windows 时启动界面' -w $settingsWindow
    $startupAfter = [Microsoft.Win32.Registry]::GetValue('HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run', 'CodexMixin', $null)
    if (($null -ne $startupAfter) -eq ($null -ne $startupValue)) { throw 'Startup checkbox did not change the native Run value' }
    & $WinApp ui click '登录 Windows 时启动界面' -w $settingsWindow
    $startupRestored = [Microsoft.Win32.Registry]::GetValue('HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run', 'CodexMixin', $null)
    if (($null -ne $startupRestored) -ne ($null -ne $startupValue)) { throw 'Startup checkbox did not restore its original enabled state' }
    $automatic = '自动更新（每小时检查；网关空闲 30 分钟后安装）'
    & $WinApp ui click $automatic -w $settingsWindow
    $automaticAfter = [Microsoft.Win32.Registry]::GetValue('HKEY_CURRENT_USER\Software\CodexMixin\Updates', 'Automatic', $null)
    $automaticBefore = if ($null -eq $automaticValue) { 1 } else { [int]$automaticValue }
    if ($null -eq $automaticAfter -or [int]$automaticAfter -eq $automaticBefore) { throw 'Automatic-update checkbox did not save the new state' }
    & $WinApp ui click $automatic -w $settingsWindow
    if ([int][Microsoft.Win32.Registry]::GetValue('HKEY_CURRENT_USER\Software\CodexMixin\Updates', 'Automatic', -1) -ne $automaticBefore) { throw 'Automatic-update checkbox did not restore its enabled state' }
    & $WinApp ui invoke 运行诊断 -w $settingsWindow
    & $WinApp ui wait-for operation-status -w $settingsWindow --value '诊断完成' --contains --timeout 90000
    $null = & $cli --no-tui --json-errors doctor --json | ConvertFrom-Json
    & $WinApp ui invoke 导出配置 -w $settingsWindow
    & $WinApp ui wait-for CloseButton -w $settingsWindow --timeout 5000
    & $WinApp ui invoke CloseButton -w $settingsWindow
    if (-not (Test-Path $env:CODEX_GATEWAY_CONFIG)) { throw 'Backup cancellation removed configuration' }
    $stage = 'native backup export, import and restoration'
    $backup = Join-Path $fixture 'config.b64'
    $beforeBackup = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    & $WinApp ui invoke 导出配置 -w $settingsWindow
    & $WinApp ui invoke PrimaryButton -w $settingsWindow
    & $WinApp ui wait-for 'File name:' -a $app.Id --timeout 30000
    & $WinApp ui inspect -a $app.Id --depth 10 --json | Set-Content "$evidence\export-picker-tree.json"
    $field = & $WinApp ui search 'File name:' -a $app.Id --type Edit --json | ConvertFrom-Json
    & $WinApp ui set-value $field.matches[0].selector $backup -a $app.Id
    $button = & $WinApp ui search Save -a $app.Id --type Button --json | ConvertFrom-Json
    & $WinApp ui invoke $button.matches[0].selector -a $app.Id
    & $WinApp ui wait-for operation-status -w $settingsWindow --value '导出配置完成' --contains --timeout 90000
    if (-not (Test-Path $backup) -or (Get-Item $backup).Length -eq 0) { throw 'Native save picker did not export the fixture' }
    & $cli --no-tui --json-errors provider update ui-local --display-name 'Changed after backup'
    & $WinApp ui invoke 导入配置 -w $settingsWindow
    & $WinApp ui wait-for 'File name:' -a $app.Id --timeout 30000
    $field = & $WinApp ui search 'File name:' -a $app.Id --type Edit --json | ConvertFrom-Json
    & $WinApp ui set-value $field.matches[0].selector $backup -a $app.Id
    $button = & $WinApp ui search Open -a $app.Id --type Button --json | ConvertFrom-Json
    & $WinApp ui invoke $button.matches[0].selector -a $app.Id
    & $WinApp ui wait-for PrimaryButton -w $settingsWindow --timeout 15000
    & $WinApp ui invoke PrimaryButton -w $settingsWindow
    & $WinApp ui wait-for operation-status -w $settingsWindow --value '导入配置完成' --contains --timeout 90000
    $afterBackup = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    if ($afterBackup.providers[0].display_name -ne $beforeBackup.providers[0].display_name) { throw 'Native import did not restore the exported provider' }
    Remove-Item $backup
    $completed.Add($stage)
    & $WinApp ui inspect -w $settingsWindow --depth 10 --json | Set-Content "$evidence\settings-tree.json"
    & $WinApp ui send-keys alt+f4 -w $settingsWindow --via send-input --allow-system-keys
    $completed.Add($stage)

    $stage = 'tray, gateway toggle, refresh, console reopening and exit'
    $null = Start-Process (Join-Path $AppDirectory 'codex_mixin_ui.exe') -ArgumentList '--page=overview' -PassThru
    & $WinApp ui wait-for tray-refresh -a $app.Id --timeout 30000
    $tree = & $WinApp ui inspect -a $app.Id --json | ConvertFrom-Json
    $trayWindow = @($tree.windows | Where-Object title -eq 'Codex Mixin · 主面板')[0].hwnd
    & $WinApp ui invoke tray-refresh -w $trayWindow
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    do {
        $switch = & $WinApp ui search 本地网关 -w $trayWindow --json | ConvertFrom-Json
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Gateway switch did not become ready' }
        if (-not $switch.matches[0].isEnabled) { Start-Sleep -Milliseconds 100 }
    } until ($switch.matches[0].isEnabled)
    $before = & $cli --no-tui --json-errors service status --json | ConvertFrom-Json
    & $WinApp ui invoke 本地网关 -w $trayWindow --action toggle
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    do {
        $after = & $cli --no-tui --json-errors service status --json | ConvertFrom-Json
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Gateway switch did not change actual service state' }
        if ($after.gateway -eq $before.gateway) { Start-Sleep -Milliseconds 200 }
    } until ($after.gateway -ne $before.gateway)
    & $WinApp ui screenshot -w $trayWindow --output "$evidence\tray.png"
    & $WinApp ui invoke '打开控制台…' -w $trayWindow
    & $WinApp ui wait-for console-providers-tab -a $app.Id --timeout 15000
    & $WinApp ui invoke console-providers-tab -a $app.Id --action select
    & $WinApp ui wait-for operation-status -a $app.Id --value '切换页面完成' --contains --timeout 90000
    & $WinApp ui invoke 删除服务商 -a $app.Id
    & $WinApp ui invoke CloseButton -a $app.Id
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    if (@($listed.providers | Where-Object id -eq 'ui-local').Count -ne 1) { throw 'Cancelled provider deletion removed configuration' }
    & $WinApp ui invoke 删除服务商 -a $app.Id
    & $WinApp ui invoke PrimaryButton -a $app.Id
    & $WinApp ui wait-for operation-status -a $app.Id --value '删除服务商完成' --contains --timeout 90000
    $listed = & $cli --no-tui --json-errors provider list --json | ConvertFrom-Json
    if (@($listed.providers).Count -ne 0) { throw 'Provider delete button did not remove the isolated provider' }
    $completed.Add('provider delete cancellation and confirmation')
    $null = Start-Process (Join-Path $AppDirectory 'codex_mixin_ui.exe') -ArgumentList '--page=overview' -PassThru
    & $WinApp ui wait-for tray-refresh -a $app.Id --timeout 15000
    & $WinApp ui invoke '退出 Codex Mixin' -w $trayWindow
    & $WinApp ui wait-for CloseButton -w $trayWindow --timeout 5000
    & $WinApp ui invoke CloseButton -w $trayWindow
    if ($app.HasExited) { throw 'Cancel on the quit confirmation exited the application' }
    & $WinApp ui invoke '退出 Codex Mixin' -w $trayWindow
    & $WinApp ui wait-for PrimaryButton -w $trayWindow --timeout 5000
    & $WinApp ui invoke PrimaryButton -w $trayWindow
    if (-not $app.WaitForExit(15000)) { throw 'Quit button did not exit the application' }
    $completed.Add($stage)
    @{ passed = $completed.ToArray(); phase = 'native UI and CLI'; notCovered = @('provider OAuth and credential-dependent adapters', 'installing an application update', 'ECH network checks') } | ConvertTo-Json -Depth 6 | Set-Content "$evidence\coverage.json"
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
        if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force -ErrorAction Continue }
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
