param(
  [string]$Source = "$(Join-Path $PSScriptRoot '..\..\macos\CodexMixin.icon\Assets\CodexMixin.png')"
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

function New-IcoFile {
  param(
    [Parameter(Mandatory)] [string]$InputPath,
    [Parameter(Mandatory)] [string]$OutputPath,
    [int[]]$Sizes = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
  )

  $sourceImage = [System.Drawing.Image]::FromFile((Resolve-Path $InputPath))
  $frames = @()
  try {
    foreach ($size in $Sizes) {
      $bitmap = New-Object System.Drawing.Bitmap $size, $size
      $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
      try {
        $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
        $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
        $graphics.DrawImage($sourceImage, 0, 0, $size, $size)
        $stream = New-Object System.IO.MemoryStream
        $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
        $frames += ,$stream.ToArray()
        $stream.Dispose()
      } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
      }
    }
  } finally {
    $sourceImage.Dispose()
  }

  $directory = Split-Path -Parent $OutputPath
  New-Item -ItemType Directory -Force -Path $directory | Out-Null
  $file = [System.IO.File]::Create($OutputPath)
  $writer = New-Object System.IO.BinaryWriter $file
  try {
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$frames.Count)
    $offset = 6 + (16 * $frames.Count)
    for ($index = 0; $index -lt $frames.Count; $index++) {
      $size = $Sizes[$index]
      $writer.Write([byte]$(if ($size -eq 256) { 0 } else { $size }))
      $writer.Write([byte]$(if ($size -eq 256) { 0 } else { $size }))
      $writer.Write([byte]0)
      $writer.Write([byte]0)
      $writer.Write([uint16]1)
      $writer.Write([uint16]32)
      $writer.Write([uint32]$frames[$index].Length)
      $writer.Write([uint32]$offset)
      $offset += $frames[$index].Length
    }
    foreach ($frame in $frames) {
      $writer.Write($frame)
    }
  } finally {
    $writer.Dispose()
    $file.Dispose()
  }
}

$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
New-IcoFile -InputPath $Source -OutputPath (Join-Path $repo "windows\assets\tray_icon.ico")
New-IcoFile -InputPath $Source -OutputPath (Join-Path $repo "windows\windows\runner\resources\app_icon.ico")

# Provider icons live under macos/assets/providers as the single source of
# truth. Flutter can only bundle assets inside its own package root, so mirror
# them into windows/assets/providers here (that directory is gitignored and
# regenerated) instead of committing a duplicate copy.
$providerSrc = Join-Path $repo "macos\assets\providers"
$providerDst = Join-Path $repo "windows\assets\providers"
New-Item -ItemType Directory -Force -Path $providerDst | Out-Null
Copy-Item -Path (Join-Path $providerSrc "*.svg") -Destination $providerDst -Force

Write-Host "Generated Windows assets (icons + provider SVGs) from $Source and $providerSrc"
