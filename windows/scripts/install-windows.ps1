param(
  [string]$InstallRoot = "$(Join-Path $env:LOCALAPPDATA 'CodexMixin')"
)

$ErrorActionPreference = "Stop"
$source = Split-Path -Parent $MyInvocation.MyCommand.Path
$resolvedInstallRoot = [System.IO.Path]::GetFullPath($InstallRoot)

# Stop any UI or gateway left running by a previous install so Windows releases
# the file locks on the executables before we overwrite them.
$existingCli = Join-Path $resolvedInstallRoot "codex-mixin.exe"
if (Test-Path -LiteralPath $existingCli) {
  try { & $existingCli service stop | Out-Null } catch {}
}
foreach ($processName in @("codex_mixin_ui", "codex-mixin")) {
  Get-Process -Name $processName -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($resolvedInstallRoot, [System.StringComparison]::OrdinalIgnoreCase) } |
    ForEach-Object {
      try { Stop-Process -Id $_.Id -Force -ErrorAction Stop } catch {}
    }
}
Start-Sleep -Milliseconds 500

New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
Get-ChildItem -LiteralPath $source | Where-Object {
  $_.Name -notin @("install-windows.ps1", "uninstall-windows.ps1", "SOURCE-README.md")
} | Copy-Item -Destination $InstallRoot -Recurse -Force

$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
$shortcutPath = Join-Path $startMenu "Codex Mixin.lnk"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = Join-Path $InstallRoot "codex_mixin_ui.exe"
$shortcut.WorkingDirectory = $InstallRoot
$shortcut.Description = "Codex Mixin Windows control center"
$shortcut.Save()

& (Join-Path $source "update-user-path.ps1") -InstallRoot $resolvedInstallRoot
Write-Host "Added $resolvedInstallRoot to your user PATH (open a new shell to use codex-mixin)."

# Harden the initial config directory before first launch. The Rust core also
# enforces this ACL whenever it creates a sensitive directory or file, so CLI
# and portable-package users receive the same protection.
$configDir = Join-Path $env:USERPROFILE ".codex-mixin"
New-Item -ItemType Directory -Force -Path $configDir | Out-Null
$me = if ($env:USERDOMAIN) { "$env:USERDOMAIN\$env:USERNAME" } else { $env:USERNAME }
& icacls.exe $configDir /inheritance:r /grant:r "${me}:(OI)(CI)F" "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" | Out-Null

Write-Host "Installed Codex Mixin to $InstallRoot"
Write-Host "Start Menu shortcut: $shortcutPath"
Write-Host "The UI starts the local Rust gateway and manages Codex configuration."
