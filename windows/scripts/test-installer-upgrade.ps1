param([Parameter(Mandatory = $true)][string]$Installer)
$ErrorActionPreference = 'Stop'
$Installer = (Resolve-Path -LiteralPath $Installer).Path
$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexMixin'
foreach ($view in @([Microsoft.Win32.RegistryView]::Registry64, [Microsoft.Win32.RegistryView]::Registry32)) {
    $registry = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, $view)
    try {
        $existing = $registry.OpenSubKey($uninstallKey)
        if ($existing) { $existing.Dispose(); throw 'Installer regression requires a clean CI runner without Codex Mixin installed.' }
    } finally { $registry.Dispose() }
}
$directory = Join-Path $env:RUNNER_TEMP ('codex-mixin-install-' + [Guid]::NewGuid().ToString('N'))
$startupKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$previous = Get-ItemPropertyValue -LiteralPath $startupKey -Name CodexMixin -ErrorAction SilentlyContinue
try {
    for ($pass = 0; $pass -lt 2; $pass++) {
        $process = Start-Process -FilePath $Installer -ArgumentList @('/S', "/D=$directory") -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw "Installer pass $pass failed (exit $($process.ExitCode))." }
        foreach ($name in @('codex_mixin_ui.exe', 'codex-mixin.exe', 'uninstall.exe')) {
            if (-not (Test-Path -LiteralPath (Join-Path $directory $name))) { throw "Complete bundle is missing $name after pass $pass." }
        }
        if ($pass -eq 0) {
            New-Item -Path $startupKey -Force | Out-Null
            Set-ItemProperty -LiteralPath $startupKey -Name CodexMixin -Value ('"' + (Join-Path $directory 'codex_mixin_ui.exe') + '" --background')
        } elseif ((Get-ItemPropertyValue -LiteralPath $startupKey -Name CodexMixin) -ne ('"' + (Join-Path $directory 'codex_mixin_ui.exe') + '" --background')) {
            throw 'Upgrade lost or changed the GUI startup choice.'
        }
    }
    Write-Output 'PASS: complete install and upgrade preserve GUI startup.'
} finally {
    $uninstaller = Join-Path $directory 'uninstall.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        $process = Start-Process -FilePath $uninstaller -ArgumentList @('/S', "_?=$directory") -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw "Installer cleanup failed (exit $($process.ExitCode))." }
    }
    if ($null -ne $previous) { Set-ItemProperty -LiteralPath $startupKey -Name CodexMixin -Value $previous }
    else { Remove-ItemProperty -LiteralPath $startupKey -Name CodexMixin -ErrorAction SilentlyContinue }
    if (Test-Path -LiteralPath $directory) { Remove-Item -LiteralPath $directory -Recurse -Force }
}
