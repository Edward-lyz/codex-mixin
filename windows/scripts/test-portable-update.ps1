$ErrorActionPreference = 'Stop'
$root = Join-Path $env:RUNNER_TEMP ('codex-mixin-portable-' + [Guid]::NewGuid().ToString('N'))
$base = Join-Path $root 'app'
$payload = Join-Path $root 'payload'
$helper = Join-Path $PSScriptRoot 'install-update.ps1'
$powershell = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
try {
    New-Item -ItemType Directory -Path $base, $payload | Out-Null
    foreach ($name in @('codex_mixin_ui.exe', 'codex-mixin.exe', 'WinSparkle.dll')) { Set-Content -LiteralPath (Join-Path $base $name) -Value 'old'; Set-Content -LiteralPath (Join-Path $payload $name) -Value 'new' }
    Set-Content -LiteralPath (Join-Path $base 'user-notes.txt') -Value 'keep'
    # A signed but incomplete release must fail without replacing any old file.
    Remove-Item -LiteralPath (Join-Path $payload 'WinSparkle.dll')
    for ($pass = 0; $pass -lt 2; $pass++) {
        if ($pass -eq 1) { Set-Content -LiteralPath (Join-Path $payload 'WinSparkle.dll') -Value 'new' }
        $package = Join-Path $root "update-$pass.zip"
        Compress-Archive -Path (Join-Path $payload '*') -DestinationPath $package
        $job = Join-Path $root 'job.json'
        @{ directory = $base; package = $package; installed = $false; gatewayRunning = $false; pid = 0 } | ConvertTo-Json | Set-Content -LiteralPath $job
        $process = Start-Process -FilePath $powershell -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', "`"$helper`"", '-Job', "`"$job`"", '-NoRelaunch') -Wait -PassThru
        if (($pass -eq 0 -and $process.ExitCode -eq 0) -or ($pass -eq 1 -and $process.ExitCode -ne 0)) { throw "Unexpected portable updater exit: $($process.ExitCode)" }
        $expected = if ($pass -eq 0) { 'old' } else { 'new' }
        if ((Get-Content -LiteralPath (Join-Path $base 'codex_mixin_ui.exe') -Raw).Trim() -ne $expected) { throw 'Portable directory replacement was not atomic.' }
        if ((Get-Content -LiteralPath (Join-Path $base 'user-notes.txt') -Raw).Trim() -ne 'keep') { throw 'Portable update lost an unknown user file.' }
    }
    Write-Output 'PASS: incomplete portable updates preserve the app; complete updates replace the bundle and preserve user files.'
} finally { if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force } }
