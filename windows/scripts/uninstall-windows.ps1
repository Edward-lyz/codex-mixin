param(
  [string]$InstallRoot = "$(Join-Path $env:LOCALAPPDATA 'CodexMixin')"
)

$ErrorActionPreference = "Stop"
$resolvedInstallRoot = [System.IO.Path]::GetFullPath($InstallRoot)
$installRootName = $resolvedInstallRoot.TrimEnd("\")
$filesystemRoot = [System.IO.Path]::GetPathRoot($resolvedInstallRoot).TrimEnd("\")
if ([string]::IsNullOrWhiteSpace($installRootName) -or $installRootName -eq $filesystemRoot) {
  throw "Refusing to remove a filesystem root: $resolvedInstallRoot"
}
$uiBinary = Join-Path $resolvedInstallRoot "codex_mixin_ui.exe"
$cliBinary = Join-Path $resolvedInstallRoot "codex-mixin.exe"
if (-not (Test-Path -LiteralPath $uiBinary -PathType Leaf) -or
    -not (Test-Path -LiteralPath $cliBinary -PathType Leaf)) {
  throw "Refusing to remove a directory that is not a Codex Mixin installation: $resolvedInstallRoot"
}
$shortcutPath = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Codex Mixin.lnk"
$runKey = "HKCU\Software\Microsoft\Windows\CurrentVersion\Run"

# Stop the gateway and UI before deleting files, otherwise Windows keeps the
# running executables locked and the removal below fails half-way.
try { & $cliBinary service stop | Out-Null } catch {}
foreach ($processName in @("codex_mixin_ui", "codex-mixin")) {
  Get-Process -Name $processName -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($resolvedInstallRoot, [System.StringComparison]::OrdinalIgnoreCase) } |
    ForEach-Object {
      try { Stop-Process -Id $_.Id -Force -ErrorAction Stop } catch {}
    }
}
Start-Sleep -Milliseconds 500

if (Test-Path -LiteralPath $shortcutPath) {
  Remove-Item -LiteralPath $shortcutPath -Force
}
& reg.exe delete $runKey /v CodexMixin /f | Out-Null
if (Test-Path -LiteralPath $resolvedInstallRoot) {
  Remove-Item -LiteralPath $resolvedInstallRoot -Recurse -Force
}

# Remove the install directory from the user's PATH if the installer added it.
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath) {
  $target = $resolvedInstallRoot.TrimEnd("\")
  $kept = $userPath.Split(";", [StringSplitOptions]::RemoveEmptyEntries) |
    Where-Object { $_.TrimEnd("\") -ne $target }
  [Environment]::SetEnvironmentVariable("Path", ($kept -join ";"), "User")
}

Write-Host "Codex Mixin binaries and shortcut removed."
Write-Host "User configuration under $env:USERPROFILE\.codex-mixin was preserved."
