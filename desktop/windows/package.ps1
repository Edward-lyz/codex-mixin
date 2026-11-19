# Builds the Windows packages: the GPUI desktop shell with the Rust CLI beside
# it, where the shell looks for codex-mixin.exe. Produces the portable zip,
# the in-app update archive (one top-level "Codex Mixin" folder) and, with
# -Installer, the per-user NSIS setup.
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$OutDirectory = "dist/release",
    [switch]$Installer
)
$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "../..")
Push-Location $root
try {
    cargo build --release --locked --target $Target --bin codex-mixin
    if ($LASTEXITCODE -ne 0) { throw "CLI build failed" }
    cargo build --release --target $Target --manifest-path desktop/Cargo.toml
    if ($LASTEXITCODE -ne 0) { throw "desktop build failed" }

    $version = (cargo pkgid --package codex-mixin) -replace '.*[#@]', ''
    $packageRoot = Join-Path $root "target/package/codex-mixin-$version-$Target"
    $staging = Join-Path $packageRoot "Codex Mixin"
    if (Test-Path $packageRoot) { Remove-Item -Recurse -Force $packageRoot }
    New-Item -ItemType Directory -Force -Path $staging, $OutDirectory | Out-Null

    Copy-Item "desktop/target/$Target/release/codex-mixin-desktop.exe" (Join-Path $staging "Codex Mixin.exe")
    Copy-Item "target/$Target/release/codex-mixin.exe" (Join-Path $staging "codex-mixin.exe")
    Copy-Item "desktop/windows/update-user-path.ps1" $staging
    Copy-Item README.md, LICENSE, NOTICE $staging

    $archive = Join-Path $OutDirectory "codex-mixin-$version-$Target.zip"
    if (Test-Path $archive) { Remove-Item -Force $archive }
    Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $archive
    Write-Host "Built $archive"

    # The updater replaces the installed folder with the archive's single
    # top-level entry.
    $update = Join-Path $OutDirectory "codex-mixin-app-$version-$Target.zip"
    if (Test-Path $update) { Remove-Item -Force $update }
    Compress-Archive -Path $staging -DestinationPath $update
    Write-Host "Built $update"

    if ($Installer) {
        $makensis = @(
            "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
            "${env:ProgramFiles}\NSIS\makensis.exe"
        ) | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
        if (-not $makensis) { throw "makensis.exe was not found; run install-nsis.ps1 first" }
        $setup = Join-Path (Resolve-Path $OutDirectory) "codex-mixin-$version-windows-x86_64-setup.exe"
        # NSIS needs a four-part numeric version for the file version resource.
        $numeric = ($version -replace '[^0-9.].*$', '')
        while (($numeric.Split('.')).Count -lt 4) { $numeric = "$numeric.0" }
        & $makensis /INPUTCHARSET UTF8 "/DVERSION=$numeric" "/DDISPLAY_VERSION=$version" `
            "/DSTAGING=$staging" "/DOUTFILE=$setup" `
            "/DICONFILE=$(Join-Path $root 'desktop/windows/CodexMixin.ico')" `
            (Join-Path $root "desktop/windows/installer.nsi")
        if ($LASTEXITCODE -ne 0) { throw "NSIS packaging failed" }
        Write-Host "Built $setup"
    }
}
finally {
    Pop-Location
}
