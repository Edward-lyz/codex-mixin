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
    & $WinApp ui wait-for console-pages -a $app.Id --timeout 60000
    & $WinApp ui wait-for operation-status -a $app.Id --value '连接 CLI完成' --contains --timeout 90000
    & $WinApp ui inspect -a $app.Id --json | Set-Content "$evidence\initial-tree.json"
    & $WinApp ui screenshot -a $app.Id --output "$evidence\initial.png"
    & $WinApp ui invoke 新增服务商 -a $app.Id
    & $WinApp ui inspect -a $app.Id --json | Set-Content "$evidence\add-provider-tree.json"
    & $WinApp ui screenshot -a $app.Id --output "$evidence\add-provider.png"
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
        & $WinApp ui inspect -a $app.Id --json 2> "$evidence\inspect-error.txt" | Set-Content "$evidence\final-tree.json"
        & $WinApp ui screenshot -a $app.Id --output "$evidence\final.png" 2> "$evidence\screenshot-error.txt"
        if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force -ErrorAction Continue }
    }
    & $cli --no-tui --json-errors service stop --force --allow-codex-disconnect --restore-clients *> "$evidence\service-cleanup.txt"
    if ($null -ne $upstream -and -not $upstream.HasExited) { Stop-Process -Id $upstream.Id -Force -ErrorAction Continue }
    $guiLogs = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'CodexMixin/logs'
    if (Test-Path $guiLogs) { Copy-Item $guiLogs "$evidence\gui-logs" -Recurse }
    & $WinApp ui yield *> $null
    foreach ($entry in $originalEnvironment.GetEnumerator()) {
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
    }
    Remove-Item $fixture -Recurse -Force
}
