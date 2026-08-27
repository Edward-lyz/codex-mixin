#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/update-watchdog-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/UpdateWatchdog.swift" \
  "$ROOT_DIR/macos/tests/UpdateWatchdogTests.swift" \
  -framework Cocoa \
  -framework CryptoKit \
  -o "$TEST_BINARY"

"$TEST_BINARY"
