param(
  [string]$UiRoot = "$(Join-Path (Get-Location) 'windows')"
)

$ErrorActionPreference = "Stop"
$path = Join-Path $UiRoot "windows\flutter\ephemeral\.plugin_symlinks\window_manager\windows\window_manager.cpp"
if (-not (Test-Path -LiteralPath $path)) {
  throw "window_manager.cpp not found at $path; run 'flutter pub get' first. The tray/flyout windows call setSkipTaskbar and require this native null-guard patch."
}

$content = [System.IO.File]::ReadAllText($path)
if ($content.Contains('if (taskbar_ == nullptr)')) {
  Write-Host "window_manager SetSkipTaskbar already patched"
  return
}

$needle = "HWND hWnd = GetMainWindow();`r`n`r`n  LPVOID lp = NULL;`r`n  CoInitialize(lp);`r`n`r`n  taskbar_->HrInit();"
$unixNeedle = "HWND hWnd = GetMainWindow();`n`n  LPVOID lp = NULL;`n  CoInitialize(lp);`n`n  taskbar_->HrInit();"
$replacement = @"
HWND hWnd = GetMainWindow();
  if (hWnd == nullptr) {
    return;
  }

  if (taskbar_ == nullptr) {
    ::CoCreateInstance(CLSID_TaskbarList, NULL, CLSCTX_INPROC_SERVER,
                       IID_PPV_ARGS(&taskbar_));
  }
  if (taskbar_ == nullptr) {
    return;
  }

  taskbar_->HrInit();
"@

$updated = $content.Replace($needle, $replacement)
if ($updated -eq $content) {
  $updated = $content.Replace($unixNeedle, $replacement.Replace("`r`n", "`n"))
}
if ($updated -eq $content) {
  throw "window_manager SetSkipTaskbar source no longer matches the expected pattern; the native null-guard patch could not be applied. Update this patch for the current window_manager version before building, otherwise setSkipTaskbar can crash the tray/flyout windows."
}

if ($updated.Contains('HWND native_window;')) {
  $updated = $updated.Replace('HWND native_window;', 'HWND native_window = nullptr;')
}
[System.IO.File]::WriteAllText($path, $updated)
Write-Host "Patched window_manager SetSkipTaskbar null-guard"
