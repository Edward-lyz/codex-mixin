# Builds the portable Windows package: the GPUI desktop shell with the Rust
# CLI beside it, where the shell looks for codex-mixin.exe.
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$OutDirectory = "dist/release"
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
    $staging = Join-Path $root "target/package/codex-mixin-$version-$Target"
    if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
    New-Item -ItemType Directory -Force -Path $staging, $OutDirectory | Out-Null

    Copy-Item "desktop/target/$Target/release/codex-mixin-desktop.exe" (Join-Path $staging "Codex Mixin.exe")
    Copy-Item "target/$Target/release/codex-mixin.exe" (Join-Path $staging "codex-mixin.exe")
    Copy-Item README.md, LICENSE, NOTICE $staging

    $archive = Join-Path $OutDirectory "codex-mixin-$version-$Target.zip"
    if (Test-Path $archive) { Remove-Item -Force $archive }
    Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $archive
    Write-Host "Built $archive"
}
finally {
    Pop-Location
}
