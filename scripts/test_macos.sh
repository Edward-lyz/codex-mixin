#!/bin/bash
# Table-driven runner for non-UI macOS Swift tests.
#   ./scripts/test_macos.sh              run all tests serially
#   ./scripts/test_macos.sh --jobs 3     run independent tests with three workers
#   ./scripts/test_macos.sh quota_support run one named test
#   ./scripts/test_macos.sh --list       print test names
# agent_logos validates dist/Codex Mixin.app, so run macos/build_app.sh first.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARCH="$(uname -m)"
WORK_DIR="$(mktemp -d)"
CURRENT_TEST=""
SWIFTGEN_DONE="${CODEX_MIXIN_SWIFTGEN_READY:-0}"
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
app_operation_logging|1|||AppOperationLogging.swift tests/AppOperationLoggingTests.swift
context_settings|1|||ContextSettingsSupport.swift tests/ContextSettingsTests.swift
fusion_settings|1|||FusionSettingsLogic.swift tests/FusionSettingsLogicTests.swift
launch_agent_bootstrap|0|||LaunchAgentBootstrapSupport.swift LaunchAtLoginSupport.swift tests/LaunchAgentBootstrapTests.swift
model_change_notification|0||UserNotifications|ModelChangeNotification.swift tests/ModelChangeNotificationTests.swift
process_output_collector|0|||ProcessOutputCollector.swift tests/ProcessOutputCollectorTests.swift
provider_support|1|||ProviderModelSelectionSupport.swift ProviderSupport.swift tests/ProviderSupportTests.swift
provider_model_selection|1|||ProviderModelSelectionSupport.swift ProviderSupport.swift tests/ProviderModelSelectionTests.swift
quota_support|1|||QuotaSupport.swift ConsoleUsageSupport.swift tests/QuotaSupportTests.swift
status_refresh_coordinator|0|||RefreshTimerSupport.swift StatusRefreshCoordinator.swift tests/StatusRefreshCoordinatorTests.swift
update_watchdog|0||Cocoa CryptoKit|UpdateWatchdog.swift AppDataDirectory.swift tests/UpdateWatchdogTests.swift
silent_update|0|||SilentUpdateSupport.swift tests/SilentUpdateTests.swift
user_shell_path|0|-sdk @SDK@ -target @ARCH@-apple-macos13.1 -parse-as-library||AppDataDirectory.swift AppOperationLogging.swift UserShellPathSupport.swift tests/UserShellPathTests.swift
'
# Tests that do not fit the compile-and-run table; each has a t_<name> function.
SPECIAL="agent_logos codex_gateway_switch_state"

TEST_ENV=()   # extra KEY=VALUE pairs for the test binary, set by prep_<name>

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
      @SDK@) args+=("${SDKROOT:-$(xcrun --show-sdk-path --sdk macosx)}") ;;
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

run_parallel() {
  local jobs="$1"
  "$ROOT_DIR/scripts/swiftgen.sh" config run --config "$ROOT_DIR/swiftgen.yml" >/dev/null
  export CODEX_MIXIN_SWIFTGEN_READY=1
  all_names | xargs -n 1 -P "$jobs" "$0" --run-one
}

main() {
  local jobs=1 name
  case "${1:-}" in
    --list) all_names; return ;;
    "") ;;
    --jobs)
      [ "$#" -eq 2 ] && [[ "$2" =~ ^[1-9][0-9]*$ ]] || {
        echo "usage: $0 [--jobs N | --list | TEST_NAME]" >&2
        exit 2
      }
      jobs="$2"
      ;;
    --run-one)
      [ "$#" -eq 2 ] || { echo "usage: $0 --run-one TEST_NAME" >&2; exit 2; }
      all_names | grep -qx "$2" || { echo "unknown test: $2 (see --list)" >&2; exit 2; }
      run_one "$2"
      return
      ;;
    *)
      all_names | grep -qx "$1" || { echo "unknown test: $1 (see --list)" >&2; exit 2; }
      run_one "$1"
      return
      ;;
  esac
  if [ "$jobs" -gt 1 ]; then
    run_parallel "$jobs"
  else
    for name in $(all_names); do run_one "$name"; done
  fi
  CURRENT_TEST=""
  echo "macOS tests passed"
}
main "$@"
