#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/fusion-settings-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/FusionSettingsLogic.swift" \
  "$ROOT_DIR/macos/tests/FusionSettingsLogicTests.swift" \
  -o "$TEST_BINARY"
"$TEST_BINARY"
