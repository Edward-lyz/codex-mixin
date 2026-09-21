#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>
#include <stdio.h>
#include <strsafe.h>

#include "flutter/generated_plugin_registrant.h"
#include "flutter_window.h"
#include "utils.h"

#include <desktop_multi_window/desktop_multi_window_plugin.h>
#include <window_manager/window_manager_plugin.h>

namespace {

// Write a last-resort crash line so native access violations in this UI
// process are diagnosable. The log lives at
// %LOCALAPPDATA%\CodexMixin\logs\ui-crash.log.
LONG WINAPI LogUnhandledException(EXCEPTION_POINTERS* info) {
  wchar_t base[MAX_PATH] = L".";
  GetEnvironmentVariableW(L"LOCALAPPDATA", base, MAX_PATH);

  wchar_t parent[MAX_PATH];
  StringCchPrintfW(parent, _countof(parent), L"%s\\CodexMixin", base);
  CreateDirectoryW(parent, nullptr);

  wchar_t dir[MAX_PATH];
  StringCchPrintfW(dir, _countof(dir), L"%s\\CodexMixin\\logs", base);
  CreateDirectoryW(dir, nullptr);

  wchar_t path[MAX_PATH];
  StringCchPrintfW(path, _countof(path), L"%s\\ui-crash.log", dir);

  HANDLE file = CreateFileW(
      path, FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr,
      OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file != INVALID_HANDLE_VALUE) {
    DWORD code = info ? info->ExceptionRecord->ExceptionCode : 0;
    DWORD_PTR addr =
        info ? reinterpret_cast<DWORD_PTR>(info->ExceptionRecord->ExceptionAddress)
             : 0;
    char line[256];
    int len = sprintf_s(line, _countof(line),
                        "crash code=0x%08lx addr=0x%p\n", code,
                        reinterpret_cast<void*>(addr));
    if (len > 0) {
      DWORD written = 0;
      WriteFile(file, line, static_cast<DWORD>(len), &written, nullptr);
    }
    CloseHandle(file);
  }
  return EXCEPTION_EXECUTE_HANDLER;
}

}  // namespace

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  SetUnhandledExceptionFilter(LogUnhandledException);

  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }

  // Initialize COM, so that it is available for use in the library and/or
  // plugins.
  ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

  flutter::DartProject project(L"data");

  std::vector<std::string> command_line_arguments =
      GetCommandLineArguments();

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  // Sub-windows already register DesktopMultiWindow internally. Calling the
  // generated RegisterPlugins() here would attach the child engine as another
  // "main" window and crash the process on tray clicks.
  DesktopMultiWindowSetWindowCreatedCallback([](void* controller) {
    auto* flutter_controller =
        reinterpret_cast<flutter::FlutterViewController*>(controller);
    if (flutter_controller == nullptr || flutter_controller->engine() == nullptr) {
      return;
    }
    WindowManagerPluginRegisterWithRegistrar(
        flutter_controller->engine()->GetRegistrarForPlugin(
            "WindowManagerPlugin"));
  });

  FlutterWindow window(project);
  Win32Window::Point origin(10, 10);
  Win32Window::Size size(1280, 720);
  if (!window.Create(L"codex_mixin_ui", origin, size)) {
    return EXIT_FAILURE;
  }
  window.SetQuitOnClose(true);

  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0)) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  ::CoUninitialize();
  return EXIT_SUCCESS;
}
