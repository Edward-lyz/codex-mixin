$ErrorActionPreference = 'Stop'

# Chocolatey's public feed can return transient 503s. Retry installation, but
# never continue packaging after a failed install or without a usable compiler.
$attemptLimit = 3
for ($attempt = 1; $attempt -le $attemptLimit; $attempt++) {
  choco install nsis -y --no-progress
  $installExitCode = $LASTEXITCODE
  if ($installExitCode -eq 0) { break }
  if ($attempt -eq $attemptLimit) {
    throw "NSIS installation failed after $attemptLimit attempts (exit $installExitCode)."
  }
  Write-Warning "NSIS installation failed (exit $installExitCode); retrying attempt $($attempt + 1)/$attemptLimit."
  Start-Sleep -Seconds (5 * $attempt)
}

$compilerPaths = @(
  "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
  "${env:ProgramFiles}\NSIS\makensis.exe"
)
$command = Get-Command makensis -ErrorAction SilentlyContinue
if ($command) { $compilerPaths += $command.Source }
$compiler = $compilerPaths | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $compiler) { throw 'NSIS installation succeeded but makensis.exe was not found.' }
& $compiler /VERSION
if ($LASTEXITCODE -ne 0) { throw "NSIS compiler verification failed (exit $LASTEXITCODE): $compiler" }
