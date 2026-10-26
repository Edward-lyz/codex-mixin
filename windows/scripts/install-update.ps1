param([Parameter(Mandatory=$true)][string]$Job, [switch]$NoRelaunch)
$ErrorActionPreference = 'Stop'
# A PowerShell 7 parent can export incompatible module paths to Windows PowerShell.
$env:PSModulePath = "$PSHOME\Modules"
Import-Module "$PSHOME\Modules\Microsoft.PowerShell.Utility\Microsoft.PowerShell.Utility.psd1" -Force
Import-Module "$PSHOME\Modules\Microsoft.PowerShell.Archive\Microsoft.PowerShell.Archive.psd1" -Force
$jobPath = [IO.Path]::GetFullPath($Job)
$work = Split-Path -Parent $jobPath
$log = Join-Path $work 'install.log'
$settings = 'HKCU:\Software\CodexMixin\Updates'
$stage = $null
$backup = $null
$committed = $false
$exitCode = 0
$description = $null
$jobData = Get-Content -LiteralPath $jobPath -Raw | ConvertFrom-Json
$base = [IO.Path]::GetFullPath($jobData.directory).TrimEnd('\')
$ui = Join-Path $base 'codex_mixin_ui.exe'
$cli = Join-Path $base 'codex-mixin.exe'
try {
    if ([IO.Path]::GetPathRoot($base).TrimEnd('\') -eq $base.TrimEnd('\')) { throw 'Portable updates require a dedicated application directory, not a drive root.' }
    if (-not [IO.Path]::IsPathRooted($jobData.package) -or -not (Test-Path -LiteralPath $jobData.package -PathType Leaf)) { throw 'The verified update package is missing.' }
    if ($jobData.pid -gt 0) {
        $old = Get-Process -Id $jobData.pid -ErrorAction SilentlyContinue
        if ($old) { $old | Wait-Process -Timeout 30 -ErrorAction Stop }
    }
    if ($jobData.gatewayRunning) {
        & $cli --no-tui --json-errors service stop --managed --json 2>&1 | Out-File -LiteralPath $log -Append
        if ($LASTEXITCODE -ne 0) { throw "Stopping gateway failed (exit $LASTEXITCODE)." }
    }
    if ($jobData.installed) {
        # NSIS parses the entire remainder after /D as an unquoted directory.
        $installer = Start-Process -FilePath $jobData.package -ArgumentList "/S /D=$base" -Verb RunAs -Wait -PassThru
        if ($installer.ExitCode -ne 0) { throw "The full installer failed (exit $($installer.ExitCode))." }
    } else {
        # Stage beside the portable directory, on the same volume. Keep unknown
        # user files; signed package files overwrite their prior versions.
        $parent = Split-Path -Parent $base
        $suffix = [Guid]::NewGuid().ToString('N')
        $stage = Join-Path $parent ".codex-mixin-stage-$suffix"
        $backup = Join-Path $parent ".codex-mixin-previous-$suffix"
        New-Item -ItemType Directory -Path $stage | Out-Null
        Expand-Archive -LiteralPath $jobData.package -DestinationPath $stage -Force
        foreach ($name in @('codex_mixin_ui.exe', 'codex-mixin.exe', 'WinSparkle.dll')) {
            if (-not (Test-Path -LiteralPath (Join-Path $stage $name) -PathType Leaf)) { throw "Incomplete portable update: $name" }
        }
        foreach ($file in Get-ChildItem -LiteralPath $base -File -Recurse -Force) {
            $relative = $file.FullName.Substring($base.Length).TrimStart('\')
            $destination = Join-Path $stage $relative
            if (-not (Test-Path -LiteralPath $destination)) {
                New-Item -ItemType Directory -Force (Split-Path -Parent $destination) | Out-Null
                Copy-Item -LiteralPath $file.FullName -Destination $destination
            }
        }
        Move-Item -LiteralPath $base -Destination $backup
        try { Move-Item -LiteralPath $stage -Destination $base }
        catch { Move-Item -LiteralPath $backup -Destination $base; throw }
    }
    if (-not (Test-Path -LiteralPath $ui -PathType Leaf) -or -not (Test-Path -LiteralPath $cli -PathType Leaf)) { throw 'The complete application was not installed.' }
    $committed = $true
    New-Item -Path $settings -Force | Out-Null
    Set-ItemProperty -Path $settings -Name LastInstallError -Value ''
    New-ItemProperty -Path $settings -Name Attempts -Value 0 -PropertyType DWord -Force | Out-Null
} catch {
    $description = $_.Exception.Message
    $description | Out-File -LiteralPath $log -Append
    New-Item -Path $settings -Force | Out-Null
    Set-ItemProperty -Path $settings -Name LastInstallError -Value $description
    $exitCode = 1
} finally {
    if ($stage -and (Test-Path -LiteralPath $stage)) { Remove-Item -LiteralPath $stage -Recurse -Force }
    if ($jobData.gatewayRunning -and (Test-Path -LiteralPath $cli)) {
        & $cli --no-tui --json-errors service start --managed --json 2>&1 | Out-File -LiteralPath $log -Append
        if ($LASTEXITCODE -ne 0) {
            $description = "Restoring gateway failed (exit $LASTEXITCODE). See $log"
            Set-ItemProperty -Path $settings -Name LastInstallError -Value $description
            $exitCode = 1
        }
    }
    if (-not $NoRelaunch -and (Test-Path -LiteralPath $ui)) {
        if ($jobData.background) { Start-Process -FilePath $ui -WorkingDirectory $base -ArgumentList '--background' | Out-Null }
        else { Start-Process -FilePath $ui -WorkingDirectory $base | Out-Null }
    }
    if ($committed -and $backup -and (Test-Path -LiteralPath $backup)) { Remove-Item -LiteralPath $backup -Recurse -Force }
    if ($committed) { Remove-Item -LiteralPath $jobData.package -Force }
}
exit $exitCode
