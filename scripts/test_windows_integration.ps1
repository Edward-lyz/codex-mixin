param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("integration-a", "integration-b", "integration-c")]
    [string] $Shard
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $root 'tests/windows_integration_shards.txt'
$rows = Get-Content -LiteralPath $manifest |
    Where-Object { $_.Trim() -and -not $_.Trim().StartsWith('#') } |
    ForEach-Object {
        $fields = $_.Trim().Split('|')
        if ($fields.Count -ne 2) {
            throw "Invalid Windows integration shard row: $_"
        }
        [pscustomobject]@{ Shard = $fields[0]; Test = $fields[1] }
    }

$arguments = @('test', '--locked')
$selected = @($rows | Where-Object { $_.Shard -eq $Shard })
if ($selected.Count -eq 0) {
    throw "Windows integration shard has no tests: $Shard"
}
foreach ($row in $selected) {
    $source = Join-Path $root "tests/$($row.Test)"
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Windows integration test is missing: $($row.Test)"
    }
    $arguments += @('--test', [System.IO.Path]::GetFileNameWithoutExtension($row.Test))
}

Push-Location $root
try {
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
} finally {
    Pop-Location
}
