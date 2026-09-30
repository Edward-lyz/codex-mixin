#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TEST_DIR"' EXIT

# The production source also contains the AppDelegate orchestration. Extract the
# Foundation-only status model so this test stays independent of AppKit UI.
sed '/^extension AppDelegate {/,$d' \
  "$ROOT_DIR/macos/CodexGatewaySwitchSupport.swift" > "$TEST_DIR/SwitchState.swift"

xcrun swiftc \
  "$TEST_DIR/SwitchState.swift" \
  "$ROOT_DIR/macos/tests/CodexGatewaySwitchStateTests.swift" \
  -o "$TEST_DIR/codex-gateway-switch-state-tests"
"$TEST_DIR/codex-gateway-switch-state-tests"
