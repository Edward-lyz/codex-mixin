param(
  # Final assembled app directory (staged, then bundled by the installer).
  [string]$Output = "$(Join-Path (Get-Location) 'dist\codex-mixin-windows')",
  # Path of the produced standard Windows installer (.exe).
  [string]$Installer = "$(Join-Path (Get-Location) 'dist\CodexMixin-Setup.exe')",
  # 4-part installer version (x.x.x.x).
  [string]$Version = "1.0.0.0",
  # Toggle Authenticode signing of the executables and the installer.
  [switch]$Sign,
  # Baidu signing tool and its options (only used when -Sign is set).
  [string]$SignTool = 'D:\work\Packet\sign\sign.exe',
  [int]$SignCert = 1,
  [string]$SignSha = 'sha256',
  # NSIS makensis.exe; auto-detected under Program Files when not provided.
  [string]$NsisPath = ''
)

$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

function Invoke-Sign {
  param([string]$Path)
  if (-not $Sign) { return }
  if (-not (Test-Path -LiteralPath $SignTool)) { throw "Signing requested but sign tool not found: $SignTool" }
  Write-Host "Signing $Path (cert=$SignCert sha=$SignSha)..."
  & $SignTool -c $SignCert -s $SignSha $Path
  if ($LASTEXITCODE -ne 0) { throw "Signing failed ($LASTEXITCODE): $Path" }
}

function Resolve-MakeNsis {
  if ($NsisPath -and (Test-Path -LiteralPath $NsisPath)) { return $NsisPath }
  foreach ($p in @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "${env:ProgramFiles}\NSIS\makensis.exe")) {
    if ($p -and (Test-Path -LiteralPath $p)) { return $p }
  }
  $cmd = Get-Command makensis -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  throw "makensis.exe not found. Install NSIS (https://nsis.sourceforge.io/Download) or pass -NsisPath."
}

Write-Host "== Building Rust CLI and Flutter UI =="
Push-Location $repo
try {
  & (Join-Path $PSScriptRoot "generate-windows-assets.ps1")
  cargo build --locked --release
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
  $uiPublish = Join-Path $repo "windows\build\windows\x64\runner\Release"
  if (Test-Path -LiteralPath $uiPublish) { Remove-Item -LiteralPath $uiPublish -Recurse -Force }
  Push-Location (Join-Path $repo "windows")
  try {
    flutter pub get
    & (Join-Path $PSScriptRoot "patch-window-manager.ps1") -UiRoot (Join-Path $repo "windows")
    & (Join-Path $PSScriptRoot "patch-multi-window-rounding.ps1") -UiRoot (Join-Path $repo "windows")
    flutter analyze
    if ($LASTEXITCODE -ne 0) { throw "flutter analyze failed" }
    flutter test
    if ($LASTEXITCODE -ne 0) { throw "flutter test failed" }
    flutter build windows --release
    if ($LASTEXITCODE -ne 0) { throw "flutter build failed" }
  } finally { Pop-Location }
} finally { Pop-Location }
Write-Host "== Assembling app directory: $Output =="
if (Test-Path -LiteralPath $Output) { Remove-Item -LiteralPath $Output -Recurse -Force }
New-Item -ItemType Directory -Path $Output | Out-Null

Copy-Item -LiteralPath (Join-Path $repo "target\release\codex-mixin.exe") -Destination $Output
Copy-Item -Path (Join-Path $uiPublish "*") -Destination $Output -Recurse -Force

$cliExe = Join-Path $Output "codex-mixin.exe"
$uiExe = Join-Path $Output "codex_mixin_ui.exe"
if (-not (Test-Path -LiteralPath $uiExe)) { throw "Windows UI executable was not included in the package" }
if (-not (Test-Path -LiteralPath $cliExe)) { throw "Rust CLI executable was not included in the package" }

# Bundle bzip2.exe (plus any DLLs it needs) so Windows tar.exe can open the
# bzip2-compressed DUCX archive at runtime without Git Bash/MSYS2. Drop a
# self-contained bzip2.exe under windows/vendor/; its whole folder is copied
# next to the app executables and located via PATH by
# src/cli/ducx_setup_windows.rs.
$vendor = Join-Path $PSScriptRoot "..\vendor"
if (Test-Path -LiteralPath $vendor) {
  Copy-Item -Path (Join-Path $vendor "*") -Destination $Output -Recurse -Force
}
if (-not (Test-Path -LiteralPath (Join-Path $Output "bzip2.exe"))) {
  throw "bzip2.exe missing from the package: place a self-contained bzip2.exe under windows/vendor/ (Windows tar.exe needs it to extract the DUCX .tar.bz2)"
}

# Sign every shipped executable and library before packaging so no bundled
# binary is left unsigned (Flutter ships flutter_windows.dll and one DLL per
# plugin alongside the two app executables). The installer itself is signed
# last, after these are embedded.
if ($Sign) {
  $signTargets = Get-ChildItem -LiteralPath $Output -Recurse -File |
    Where-Object { $_.Extension -in '.exe', '.dll' } |
    Sort-Object FullName
  if (-not $signTargets) { throw "No signable binaries found under $Output" }
  foreach ($target in $signTargets) { Invoke-Sign -Path $target.FullName }
  Write-Host ("Signed {0} bundled binaries." -f $signTargets.Count)
}

Write-Host "== Building standard Windows installer (NSIS): $Installer =="
$makensis = Resolve-MakeNsis
$nsi = Join-Path $PSScriptRoot "installer.nsi"
if (-not (Test-Path -LiteralPath $nsi)) { throw "NSIS script not found: $nsi" }
$iconFile = Join-Path $repo "windows\windows\runner\resources\app_icon.ico"
$installerDir = Split-Path -Parent $Installer
New-Item -ItemType Directory -Force -Path $installerDir | Out-Null
if (Test-Path -LiteralPath $Installer) { Remove-Item -LiteralPath $Installer -Force }

$nsisArgs = @(
  "/DVERSION=$Version",
  "/DSTAGING=$Output",
  "/DOUTFILE=$Installer"
)
if (Test-Path -LiteralPath $iconFile) { $nsisArgs += "/DICONFILE=$iconFile" }

# When signing, the uninstaller must be signed too. NSIS only produces
# uninstall.exe at install time, so use the standard two-pass trick: build a
# throwaway installer that just emits uninstall.exe, run it to write the file,
# sign it, then ship that pre-signed uninstaller in the real installer.
$presignedUninstaller = $null
if ($Sign) {
  $uninstallerOut = Join-Path $installerDir 'uninstall.exe'
  $genSetup = Join-Path $installerDir '_uninstaller_gen.exe'
  if (Test-Path -LiteralPath $uninstallerOut) { Remove-Item -LiteralPath $uninstallerOut -Force }
  if (Test-Path -LiteralPath $genSetup) { Remove-Item -LiteralPath $genSetup -Force }
  $genArgs = @(
    "/DVERSION=$Version",
    "/DSTAGING=$Output",
    "/DOUTFILE=$genSetup",
    "/DGEN_UNINSTALLER=1",
    "/DUNINST_OUT=$uninstallerOut"
  )
  if (Test-Path -LiteralPath $iconFile) { $genArgs += "/DICONFILE=$iconFile" }
  $genArgs += $nsi
  & $makensis @genArgs
  if ($LASTEXITCODE -ne 0) { throw "makensis (uninstaller generator) failed ($LASTEXITCODE)" }
  Start-Process -FilePath $genSetup -Wait
  if (-not (Test-Path -LiteralPath $uninstallerOut)) { throw "Uninstaller was not generated: $uninstallerOut" }
  Invoke-Sign -Path $uninstallerOut
  Remove-Item -LiteralPath $genSetup -Force -ErrorAction SilentlyContinue
  $presignedUninstaller = $uninstallerOut
  $nsisArgs += "/DPRESIGNED_UNINSTALLER=$uninstallerOut"
}
$nsisArgs += $nsi
& $makensis @nsisArgs
if ($LASTEXITCODE -ne 0) { throw "makensis failed ($LASTEXITCODE)" }
if (-not (Test-Path -LiteralPath $Installer)) { throw "Installer was not produced: $Installer" }
if ($presignedUninstaller) { Remove-Item -LiteralPath $presignedUninstaller -Force -ErrorAction SilentlyContinue }

# Sign the installer itself last so its embedded files stay untouched.
Invoke-Sign -Path $Installer

Write-Host ""
Write-Host "Done."
Write-Host "  App directory : $Output"
Write-Host "  Installer     : $Installer"
Write-Host ("  Signed        : {0}" -f ($Sign.IsPresent))
Write-Host "Users run the installer; it installs to %ProgramFiles%\Codex Mixin, adds a Start Menu shortcut and an Add/Remove Programs entry."

