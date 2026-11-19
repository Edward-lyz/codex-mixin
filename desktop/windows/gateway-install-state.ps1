param([Parameter(Mandatory = $true)][string]$CliPath)

$ErrorActionPreference = 'Stop'
try {
    # Only decide whether to restore the gateway; do not print configuration.
    $statusJson = & $CliPath --no-tui --json-errors service status --json
    if ($LASTEXITCODE -ne 0) { throw 'Unable to inspect gateway status.' }
    $status = $statusJson | ConvertFrom-Json
    if ($null -eq $status -or $status.configured -isnot [bool]) { throw 'Gateway status is invalid.' }
    if ($status.configured -ne $true) { exit 1 }
    $startupJson = & $CliPath --no-tui --json-errors service autostart status --json
    if ($LASTEXITCODE -ne 0) { throw 'Unable to inspect gateway autostart.' }
    $startup = $startupJson | ConvertFrom-Json
    if ($null -eq $startup -or $startup.enabled -isnot [bool]) { throw 'Gateway autostart status is invalid.' }
    if ($status.gateway -in @('running', 'unresponsive') -or $startup.enabled -eq $true) { exit 0 }
    if ($status.gateway -ne 'stopped' -or $startup.enabled -ne $false) { throw 'Gateway status is unknown.' }
    exit 1
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 2
}
