$ErrorActionPreference = 'Stop'
# A PowerShell 7 parent can export incompatible module paths to Windows PowerShell.
$env:PSModulePath = "$PSHOME\Modules"
Import-Module "$PSHOME\Modules\Microsoft.PowerShell.Utility\Microsoft.PowerShell.Utility.psd1" -Force
Import-Module "$PSHOME\Modules\Microsoft.PowerShell.Archive\Microsoft.PowerShell.Archive.psd1" -Force
$version = '0.9.4'
$expected = '6037df37fc263bd1650a1c4949681a9d40ffe991d01f35892a406cb5d103c976'
$root = Join-Path $PSScriptRoot '..\..'
$cache = Join-Path $root 'windows\build\winsparkle'
New-Item -ItemType Directory -Force $cache | Out-Null
$archive = Join-Path $cache "WinSparkle-$version.zip"
if (-not (Test-Path -LiteralPath $archive)) {
    $partial = "$archive.partial"
    try {
        Invoke-WebRequest "https://github.com/vslavik/winsparkle/releases/download/v$version/WinSparkle-$version.zip" -OutFile $partial -UseBasicParsing -TimeoutSec 120
        if ((Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'WinSparkle archive does not match the pinned release.' }
        Move-Item -LiteralPath $partial -Destination $archive -Force
    } finally { if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Force } }
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Cached WinSparkle archive is invalid.' }
Expand-Archive -LiteralPath $archive -DestinationPath $cache -Force
Copy-Item "$cache\WinSparkle-$version\x64\Release\WinSparkle.dll" "$cache\WinSparkle.dll" -Force
Copy-Item "$cache\WinSparkle-$version\COPYING" "$cache\WinSparkle.LICENSE.txt" -Force
Copy-Item "$cache\WinSparkle-$version\COPYING.expat" "$cache\WinSparkle.Expat.LICENSE.txt" -Force
$xmlSettings = New-Object System.Xml.XmlReaderSettings
$xmlSettings.DtdProcessing = [System.Xml.DtdProcessing]::Ignore
$reader = [System.Xml.XmlReader]::Create((Join-Path $root 'macos\Info.plist'), $xmlSettings)
$plist = New-Object System.Xml.XmlDocument
try { $plist.Load($reader) } finally { $reader.Dispose() }
$key = $plist.SelectSingleNode('//key[text()="SUPublicEDKey"]/following-sibling::string[1]').InnerText
if ([Convert]::FromBase64String($key).Length -ne 32) { throw 'The shared Sparkle Ed25519 public key must have 32 bytes.' }
[IO.File]::WriteAllText((Join-Path $cache 'WinSparkle.PublicKey.txt'), $key)
