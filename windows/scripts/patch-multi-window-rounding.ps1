param(
  [string]$UiRoot = "$(Join-Path (Get-Location) 'windows')"
)

$ErrorActionPreference = "Stop"

# The tray/flyout popups are desktop_multi_window sub-windows. Their native
# surface is opaque, so to get clean rounded corners (without the black corners
# that a transparent Flutter background leaves behind) we ask the Windows 11
# compositor to round the window via DWMWA_WINDOW_CORNER_PREFERENCE. DWM is
# loaded dynamically so the plugin needs no extra link dependency and the call
# is a no-op on Windows 10.
$path = Join-Path $UiRoot "windows\flutter\ephemeral\.plugin_symlinks\desktop_multi_window\windows\flutter_window.cc"
if (-not (Test-Path -LiteralPath $path)) {
  throw "desktop_multi_window flutter_window.cc not found at $path; run 'flutter pub get' first."
}

$content = [System.IO.File]::ReadAllText($path)
if ($content.Contains('DWMWA_WINDOW_CORNER_PREFERENCE')) {
  Write-Host "desktop_multi_window rounded-corner patch already applied"
  return
}

$needle = "nullptr, nullptr, GetModuleHandle(nullptr), this);"
if (-not $content.Contains($needle)) {
  throw "desktop_multi_window CreateWindow call no longer matches the expected pattern; update windows/scripts/patch-multi-window-rounding.ps1 for the current plugin version."
}

$injection = @"
nullptr, nullptr, GetModuleHandle(nullptr), this);

  // Windows 11 rounded corners for the popup sub-window. Loaded dynamically so
  // the plugin needs no dwmapi link dependency; a no-op on Windows 10.
  if (HMODULE dwm_module = LoadLibraryA("dwmapi.dll")) {
    typedef HRESULT(WINAPI * SetWindowAttributeProc)(HWND, DWORD, LPCVOID, DWORD);
    auto set_window_attribute = reinterpret_cast<SetWindowAttributeProc>(
        GetProcAddress(dwm_module, "DwmSetWindowAttribute"));
    if (set_window_attribute != nullptr) {
      DWORD corner_preference = 2;  // DWMWCP_ROUND
      set_window_attribute(window_handle, 33 /* DWMWA_WINDOW_CORNER_PREFERENCE */,
                           &corner_preference, sizeof(corner_preference));
    }
    FreeLibrary(dwm_module);
  }
"@

$updated = $content.Replace($needle, $injection)
if ($updated -eq $content) {
  throw "desktop_multi_window rounded-corner patch could not be applied."
}
[System.IO.File]::WriteAllText($path, $updated)
Write-Host "Patched desktop_multi_window with Windows 11 rounded corners"
