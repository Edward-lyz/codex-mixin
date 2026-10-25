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
New-Item -ItemType Directory -Force -Path (Join-Path $repo 'windows\assets') | Out-Null
# macOS applies its own icon mask. Its source PNG includes an opaque white
# canvas/plate, which Windows otherwise displays as a square at every size.
# Remove only edge-connected neutral background; the enclosed terminal glyph
# stays white. Derive both Windows icons from the same existing artwork.
$inputImage = [System.Drawing.Image]::FromFile((Resolve-Path $Source))
$mark = New-Object System.Drawing.Bitmap 256, 256
$graphics = [System.Drawing.Graphics]::FromImage($mark)
try {
  $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $graphics.DrawImage($inputImage, 0, 0, 256, 256)
} finally { $graphics.Dispose(); $inputImage.Dispose() }
$queue = New-Object 'System.Collections.Generic.Queue[int]'
$visited = New-Object 'bool[]' 65536
$queue.Enqueue(0)
while ($queue.Count -gt 0) {
  $index = $queue.Dequeue()
  if ($visited[$index]) { continue }
  $visited[$index] = $true
  $x = $index % 256
  $y = [int][Math]::Floor($index / 256)
  $color = $mark.GetPixel($x, $y)
  $channels = @([int]$color.R, [int]$color.G, [int]$color.B)
  $minimum = [Math]::Min($channels[0], [Math]::Min($channels[1], $channels[2]))
  $maximum = [Math]::Max($channels[0], [Math]::Max($channels[1], $channels[2]))
  if ($color.A -ne 0 -and ($minimum -lt 100 -or $maximum - $minimum -gt 40)) { continue }
  $mark.SetPixel($x, $y, [System.Drawing.Color]::Transparent)
  if ($x -gt 0) { $queue.Enqueue($index - 1) }
  if ($x -lt 255) { $queue.Enqueue($index + 1) }
  if ($y -gt 0) { $queue.Enqueue($index - 256) }
  if ($y -lt 255) { $queue.Enqueue($index + 256) }
}
$appPng = Join-Path $repo 'windows\assets\app_icon.png'
$left = 256; $top = 256; $right = -1; $bottom = -1
for ($y = 0; $y -lt 256; $y++) {
  for ($x = 0; $x -lt 256; $x++) {
    if ($mark.GetPixel($x, $y).A -eq 0) { continue }
    $left = [Math]::Min($left, $x); $right = [Math]::Max($right, $x)
    $top = [Math]::Min($top, $y); $bottom = [Math]::Max($bottom, $y)
  }
}
if ($right -lt $left) { $mark.Dispose(); throw 'Application icon has no foreground' }
$output = New-Object System.Drawing.Bitmap 256, 256
$graphics = [System.Drawing.Graphics]::FromImage($output)
try {
  $width = $right - $left + 1; $height = $bottom - $top + 1
  $scale = 240.0 / [Math]::Max($width, $height)
  $destination = New-Object System.Drawing.RectangleF ((256 - $width * $scale) / 2), ((256 - $height * $scale) / 2), ($width * $scale), ($height * $scale)
  $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $sourceBounds = New-Object System.Drawing.RectangleF $left, $top, $width, $height
  $graphics.DrawImage($mark, $destination, $sourceBounds, [System.Drawing.GraphicsUnit]::Pixel)
  $output.Save($appPng, [System.Drawing.Imaging.ImageFormat]::Png)
} finally { $graphics.Dispose(); $output.Dispose(); $mark.Dispose() }
New-IcoFile -InputPath $appPng -OutputPath (Join-Path $repo "windows\assets\tray_icon.ico")
New-IcoFile -InputPath $appPng -OutputPath (Join-Path $repo "windows\assets\app_icon.ico")
$providers = Join-Path $repo "windows\assets\providers"
New-Item -ItemType Directory -Force -Path $providers | Out-Null
Copy-Item -Path (Join-Path $repo "macos\assets\providers\*.svg") -Destination $providers -Force
$agents = Join-Path $repo "windows\assets\agents"
New-Item -ItemType Directory -Force -Path $agents | Out-Null
Copy-Item -Path (Join-Path $repo "macos\assets\agents\*.svg"), (Join-Path $repo "macos\assets\agents\*.png") -Destination $agents -Force
Copy-Item -LiteralPath (Join-Path $repo "macos\assets\agents\ATTRIBUTION.md") -Destination $agents -Force
# WinUI needs an explicit viewport for the em-sized logos used by macOS.
foreach ($asset in Get-ChildItem $agents -Filter '*.svg') {
  $document = New-Object System.Xml.XmlDocument
  $document.Load($asset.FullName)
  # SVGs such as WorkBuddy use a 64px canvas without a viewBox. Preserve
  # that coordinate system before normalizing the WinUI display size.
  if (-not $document.DocumentElement.HasAttribute('viewBox')) {
    $width = $document.DocumentElement.GetAttribute('width')
    $height = $document.DocumentElement.GetAttribute('height')
    if ($width -match '^\d+(\.\d+)?$' -and $height -match '^\d+(\.\d+)?$') {
      $document.DocumentElement.SetAttribute('viewBox', "0 0 $width $height")
    } else { throw "SVG has no numeric viewport: $($asset.Name)" }
  }
  $document.DocumentElement.SetAttribute('width', '32')
  $document.DocumentElement.SetAttribute('height', '32')
  $document.DocumentElement.SetAttribute('color', '#707070')
  $document.DocumentElement.RemoveAttribute('style')
  $document.Save($asset.FullName)
}

Write-Host "Generated native Windows application and tray icons from $Source"
