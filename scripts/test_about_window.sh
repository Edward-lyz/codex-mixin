#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/about-window-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/AboutWindow.swift" \
  "$ROOT_DIR/macos/tests/AboutWindowTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"
"$TEST_BINARY"
