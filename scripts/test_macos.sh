#!/bin/bash
# Table-driven runner for every macOS Swift test.
#   ./scripts/test_macos.sh              run all tests, stop at the first failure
#   ./scripts/test_macos.sh quota_support run one named test
#   ./scripts/test_macos.sh --list       print test names
# agent_logos validates dist/Codex Mixin.app, so run macos/build_app.sh first.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARCH="$(uname -m)"
WORK_DIR="$(mktemp -d)"
CURRENT_TEST=""
SWIFTGEN_DONE=0
on_exit() {
  local rc=$?
  rm -rf "$WORK_DIR"
  if [ "$rc" -ne 0 ]; then echo "FAILED: ${CURRENT_TEST:-setup} (exit $rc)" >&2; fi
  exit "$rc"
}
trap on_exit EXIT

# name|localization|swiftc flags|frameworks|sources (relative to macos/)
# localization=1 prepends Generated/L10n.swift + Localization.swift, runs
# swiftgen once, and exports CODEX_MIXIN_LOCALIZATION_DIR to the test binary.
# Flag tokens @SDK@ and @ARCH@ expand to the macosx SDK path and uname -m.
TABLE='
about_window|1|-target @ARCH@-apple-macosx13.1|Cocoa CryptoKit SwiftUI|ApplicationMenuSupport.swift LiquidGlassSupport.swift InstallCard.swift AppDataDirectory.swift AboutWindow.swift tests/AboutWindowTests.swift
agent_management|1|-sdk @SDK@ -target @ARCH@-apple-macos13.1|Cocoa SwiftUI|UpdateSupport.swift AppSupport.swift ApplicationMenuSupport.swift LiquidGlassSupport.swift AgentManagementSupport.swift AgentManagementWindow.swift tests/AgentManagementWindowTests.swift
app_operation_logging|1|||AppOperationLogging.swift tests/AppOperationLoggingTests.swift
application_menu|1||Cocoa|ApplicationMenuSupport.swift tests/ApplicationMenuSupportTests.swift
fusion_settings|1|||FusionSettingsLogic.swift tests/FusionSettingsLogicTests.swift
install_card|1|-target @ARCH@-apple-macosx13.1|Cocoa CryptoKit SwiftUI|ApplicationMenuSupport.swift LiquidGlassSupport.swift InstallCard.swift AppDataDirectory.swift tests/InstallCardTests.swift
install_progress_window|1|-sdk @SDK@ -target @ARCH@-apple-macos13.1|Cocoa|ApplicationMenuSupport.swift UpdateSupport.swift LiquidGlassSupport.swift AppSupport.swift InstallProgressWindow.swift tests/InstallProgressWindowTests.swift
launch_agent_bootstrap|0|||LaunchAgentBootstrapSupport.swift LaunchAtLoginSupport.swift tests/LaunchAgentBootstrapTests.swift
liquid_glass_style|0||Cocoa SwiftUI|LiquidGlassSupport.swift tests/LiquidGlassStyleTests.swift
menu_view_updates|1||Cocoa|MenuViewUpdateSupport.swift tests/MenuViewUpdateSupportTests.swift
menu_views_layout|1||Cocoa|LiquidGlassSupport.swift QuotaSupport.swift ProviderIconCache.swift AppDataDirectory.swift ServiceMenuViews.swift ProviderUsageDashboardView.swift MenuVisualSupport.swift tests/MenuViewsLayoutTests.swift
model_benchmark_data|1||Cocoa|ApplicationMenuSupport.swift UpdateSupport.swift LiquidGlassSupport.swift AppSupport.swift InstallProgressWindow.swift ProviderSupport.swift ProviderWindowLayoutSupport.swift ModelBenchmarkDataSupport.swift ModelBenchmarkWindow.swift tests/ModelBenchmarkDataTests.swift
model_change_notification|0||UserNotifications|ModelChangeNotification.swift tests/ModelChangeNotificationTests.swift
process_output_collector|0|||ProcessOutputCollector.swift tests/ProcessOutputCollectorTests.swift
provider_settings_navigation|1|-sdk @SDK@ -target @ARCH@-apple-macos26.0|Cocoa SwiftUI|ApplicationMenuSupport.swift UpdateSupport.swift AppSupport.swift InstallProgressWindow.swift SettingsPanel.swift ProviderSupport.swift ProviderIconCache.swift AppDataDirectory.swift ProviderWindowLayoutSupport.swift LiquidGlassSupport.swift ModelBenchmarkDataSupport.swift ModelBenchmarkWindow.swift ProviderSettingsView.swift AppOperationLogging.swift ProviderSettingsWindow.swift BaiduAuthBridgeSupport.swift ProviderSettingsPresentationSupport.swift tests/ProviderSettingsNavigationTests.swift
provider_support|1|||ProviderSupport.swift tests/ProviderSupportTests.swift
provider_window_layout|1||Cocoa|ApplicationMenuSupport.swift UpdateSupport.swift LiquidGlassSupport.swift AppSupport.swift InstallProgressWindow.swift ProviderSupport.swift ProviderWindowLayoutSupport.swift ModelBenchmarkDataSupport.swift ModelBenchmarkWindow.swift tests/ProviderWindowLayoutTests.swift
quota_support|1|||QuotaSupport.swift tests/QuotaSupportTests.swift
settings_panel_presentation|1||Cocoa|ApplicationMenuSupport.swift LiquidGlassSupport.swift SettingsPanel.swift tests/SettingsPanelPresentationTests.swift
status_refresh_coordinator|0|||RefreshTimerSupport.swift StatusRefreshCoordinator.swift tests/StatusRefreshCoordinatorTests.swift
update_watchdog|0||Cocoa CryptoKit|UpdateWatchdog.swift AppDataDirectory.swift tests/UpdateWatchdogTests.swift
user_shell_path|0|-sdk @SDK@ -target @ARCH@-apple-macos13.1 -parse-as-library||AppDataDirectory.swift AppOperationLogging.swift UserShellPathSupport.swift tests/UserShellPathTests.swift
'
# Tests that do not fit the compile-and-run table; each has a t_<name> function.
SPECIAL="agent_logos application_icon application_lifecycle codex_gateway_switch_state"

TEST_ENV=()   # extra KEY=VALUE pairs for the test binary, set by prep_<name>

# ---- hooks for table tests -------------------------------------------------

prep_about_window() { TEST_ENV+=("CODEX_MIXIN_WALLPAPER_ASSET_DIR=$ROOT_DIR/macos/assets/nasa-wallpapers"); }
prep_install_card() { prep_about_window; }

prep_menu_views_layout() {
  mkdir -p "$TEST_DIR/ProviderLogos"
  cp "$ROOT_DIR/macos/assets/providers/"*.svg "$TEST_DIR/ProviderLogos/"
}

prep_settings_panel_presentation() {
  cargo build --locked --manifest-path "$ROOT_DIR/Cargo.toml" >/dev/null
  "$ROOT_DIR/target/debug/codex-mixin" --no-tui interface --json > "$TEST_DIR/interface.json"
  TEST_ENV+=("CODEX_MIXIN_TEST_INTERFACE_JSON=$TEST_DIR/interface.json")
}

post_application_menu() {
  if rg -n 'NSPanel\(' "$ROOT_DIR/macos" --glob '*.swift' --glob '!**/tests/**'; then
    echo "App windows must use NSWindow so they remain visible after deactivation" >&2
    exit 1
  fi
  if rg -n 'hidesOnDeactivate\s*=\s*true' "$ROOT_DIR/macos" --glob '*.swift' --glob '!**/tests/**'; then
    echo "App windows must not hide when another app becomes active" >&2
    exit 1
  fi
  if rg -n 'makeKeyAndOrderFront|NSApp\.activate' "$ROOT_DIR/macos" \
    --glob '*.swift' \
    --glob '!ApplicationMenuSupport.swift' \
    --glob '!**/tests/**'; then
    echo "App windows must use the persistent presentation coordinator" >&2
    exit 1
  fi
}

require_text() {
  if ! grep -Fq "$2" "$1"; then
    echo "missing Liquid Glass contract in ${1#"$ROOT_DIR/"}: $2" >&2
    exit 1
  fi
}

prep_liquid_glass_style() {
  local support="$ROOT_DIR/macos/LiquidGlassSupport.swift" file
  require_text "$support" "func liquidGlassProminentButton() -> some View"
  require_text "$support" "content.buttonStyle(.glassProminent)"
  require_text "$support" "func configureOpaqueWindow(_ window: NSWindow)"
  require_text "$support" "window.isOpaque = true"
  for file in ProviderSettingsWindow FusionSettingsWindow SettingsPanel CodexConnectPanel \
    AgentManagementWindow AboutWindow InstallProgressWindow InstallCard AppSupport; do
    require_text "$ROOT_DIR/macos/$file.swift" "configureOpaqueWindow("
  done
  if grep -R -Fq --include='*.swift' --exclude='LiquidGlassSupport.swift' \
    '.liquidGlass(' "$ROOT_DIR/macos"; then
    echo "content Liquid Glass effect remains outside LiquidGlassSupport.swift" >&2
    exit 1
  fi
  if grep -R -Fq --include='*.swift' 'configureLiquidGlassWindow(' "$ROOT_DIR/macos"; then
    echo "legacy transparent window helper remains" >&2
    exit 1
  fi
  if [ "$(grep -Fc '.textFieldStyle(.roundedBorder)' "$ROOT_DIR/macos/ModelBenchmarkWindow.swift")" -ne 1 ]; then
    echo "benchmark must retain one rounded-border manual input field" >&2
    exit 1
  fi
  require_text "$ROOT_DIR/macos/ModelBenchmarkWindow.swift" "NSSearchField"
  if grep -R -Fq --include='*.swift' --exclude='LiquidGlassSupport.swift' \
    '.background(.ultraThinMaterial)' "$ROOT_DIR/macos"; then
    echo "legacy ultraThinMaterial background remains outside LiquidGlassSupport.swift" >&2
    exit 1
  fi
}
post_liquid_glass_style() { echo "Liquid Glass style contract passed"; }

# ---- table runner ------------------------------------------------------------

run_table_test() {
  local name="$1" loc="$2" flags="$3" frameworks="$4" sources="$5"
  local args=() srcs=() word fw bin="$TEST_DIR/$name-tests" bundle=""

  if [ "$loc" = 1 ]; then
    if [ "$SWIFTGEN_DONE" = 0 ]; then
      "$ROOT_DIR/scripts/swiftgen.sh" config run --config "$ROOT_DIR/swiftgen.yml" >/dev/null
      SWIFTGEN_DONE=1
    fi
    bundle="$("$ROOT_DIR/scripts/prepare_test_localization.sh")"
    srcs=("$ROOT_DIR/macos/Generated/L10n.swift" "$ROOT_DIR/macos/Localization.swift")
  fi
  if [ "$(type -t "prep_$name")" = function ]; then "prep_$name"; fi

  for word in $flags; do
    case "$word" in
      @SDK@) args+=("$(xcrun --show-sdk-path --sdk macosx)") ;;
      @ARCH@*) args+=("${ARCH}${word#@ARCH@}") ;;
      *) args+=("$word") ;;
    esac
  done
  for word in $sources; do srcs+=("$ROOT_DIR/macos/$word"); done
  for fw in $frameworks; do args+=(-framework "$fw"); done

  xcrun swiftc ${args[@]+"${args[@]}"} "${srcs[@]}" -o "$bin"
  if [ "$loc" = 1 ]; then TEST_ENV+=("CODEX_MIXIN_LOCALIZATION_DIR=$bundle"); fi
  env ${TEST_ENV[@]+"${TEST_ENV[@]}"} "$bin"
  if [ "$(type -t "post_$name")" = function ]; then "post_$name"; fi
}

# ---- special tests -------------------------------------------------------------

t_agent_logos() {
  # Validate the packaged files the actual app will load, including PNG assets.
  local logos="$ROOT_DIR/dist/Codex Mixin.app/Contents/Resources/AgentLogos"
  [ -d "$logos" ] || { echo "missing $logos; run macos/build_app.sh first" >&2; exit 1; }
  xcrun swift "$ROOT_DIR/macos/tests/AgentLogoTests.swift" "$logos"
}

t_application_icon() {
  local app="$TEST_DIR/ApplicationIconSupportTests.app"
  local contents="$app/Contents"
  mkdir -p "$contents/MacOS" "$contents/Resources"
  cp "$ROOT_DIR/macos/tests/ApplicationIconSupportTests-Info.plist" "$contents/Info.plist"
  cp "$ROOT_DIR/macos/CodexMixin.icns" "$contents/Resources/CodexMixin.icns"
  cp "$ROOT_DIR/macos/CodexMixinDark.icns" "$contents/Resources/CodexMixinDark.icns"
  xcrun swiftc \
    "$ROOT_DIR/macos/ApplicationIconSupport.swift" \
    "$ROOT_DIR/macos/tests/ApplicationIconSupportTests.swift" \
    -framework Cocoa \
    -o "$contents/MacOS/ApplicationIconSupportTests"
  "$contents/MacOS/ApplicationIconSupportTests"
}

t_codex_gateway_switch_state() {
  # The production source also contains the AppDelegate orchestration. Extract the
  # Foundation-only status model so this test stays independent of AppKit UI.
  sed '/^extension AppDelegate {/,$d' \
    "$ROOT_DIR/macos/CodexGatewaySwitchSupport.swift" > "$TEST_DIR/SwitchState.swift"
  xcrun swiftc \
    "$TEST_DIR/SwitchState.swift" \
    "$ROOT_DIR/macos/tests/CodexGatewaySwitchStateTests.swift" \
    -o "$TEST_DIR/codex-gateway-switch-state-tests"
  "$TEST_DIR/codex-gateway-switch-state-tests"
}

# Source-structure checks; they do not replace runtime/UI verification.
t_application_lifecycle() {
  local menu_bar="$ROOT_DIR/macos/MenuBarApp.swift"
  local service="$ROOT_DIR/macos/GatewayService.swift"
  local switch="$ROOT_DIR/macos/CodexGatewaySwitchSupport.swift"
  local lifecycle="$ROOT_DIR/macos/GatewayLifecycleSupport.swift"

  if rg -n 'applicationShouldTerminate' "$ROOT_DIR/macos" --glob '*.swift' --glob '!tests/**'; then
    echo "Only the menu quit actions stop the gateway; Sparkle relaunches use AppKit's default lifecycle" >&2
    exit 1
  fi
  if ! rg -q 'func applicationWillTerminate' "$menu_bar"; then
    echo "App termination must still clean up UI refresh resources" >&2
    exit 1
  fi
  if ! rg -q '\["service", "stop", "--managed", "--json"\]' "$service"; then
    echo "The explicit gateway stop action must remain available" >&2
    exit 1
  fi
  if ! rg -q 'runGatewayStreaming\(\["codex-switch", "official"\]\)' "$switch"; then
    echo "The macOS shell must delegate official-mode switching to the CLI" >&2
    exit 1
  fi
  if ! rg -q 'runGatewayStreaming\(\["codex-switch", "mixin"\]\)' "$switch"; then
    echo "The macOS shell must delegate Mixin-mode switching to the CLI" >&2
    exit 1
  fi
  if rg -q 'uninstall-codex|service", "stop|config\.toml|UserDefaults' "$switch"; then
    echo "The macOS shell must not own Codex config parsing, persistence, or gateway-stop policy" >&2
    exit 1
  fi
  if ! rg -q 'restartRunningCodexDesktopApp\(\)' "$switch"; then
    echo "Managed Codex shutdown must restart the desktop app" >&2
    exit 1
  fi
  if rg -q 'ChatchatGPT\.app' "$switch"; then
    echo "The Codex lifecycle must not restart the unrelated ChatGPT app" >&2
    exit 1
  fi
  if ! rg -q 'refreshCodexIntegrationStatus\(\)' "$lifecycle" \
    || ! rg -q 'codexStatus\.isOfficialMode' "$lifecycle"; then
    echo "App launch must use the CLI-reported Codex state before starting the gateway" >&2
    exit 1
  fi
  echo "Application exit keeps gateway running: passed"
}

# ---- dispatch --------------------------------------------------------------------

table_rows() { printf '%s\n' "$TABLE" | sed '/^$/d'; }
all_names() { { table_rows | cut -d'|' -f1; for n in $SPECIAL; do echo "$n"; done; } | sort; }

run_one() {
  local name="$1" row
  CURRENT_TEST="$name"
  TEST_DIR="$WORK_DIR/$name"
  TEST_ENV=()
  mkdir -p "$TEST_DIR"
  echo "==> $name"
  if [ "$(type -t "t_$name")" = function ]; then
    "t_$name"
    return
  fi
  row="$(table_rows | grep "^$name|")"
  IFS='|' read -r _ loc flags frameworks sources <<<"$row"
  run_table_test "$name" "$loc" "$flags" "$frameworks" "$sources"
}

main() {
  local name
  case "${1:-}" in
    --list) all_names; return ;;
    "") for name in $(all_names); do run_one "$name"; done ;;
    *)
      all_names | grep -qx "$1" || { echo "unknown test: $1 (see --list)" >&2; exit 2; }
      run_one "$1"
      ;;
  esac
  CURRENT_TEST=""
  echo "macOS tests passed"
}
main "$@"
