#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/model-change-notification-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/ModelChangeNotification.swift" \
  "$ROOT_DIR/macos/tests/ModelChangeNotificationTests.swift" \
  -framework UserNotifications \
  -o "$TEST_BINARY"
"$TEST_BINARY"
