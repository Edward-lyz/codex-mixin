#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/user-shell-path-tests"

xcrun swiftc \
  -sdk "$(xcrun --show-sdk-path --sdk macosx)" \
  -target "$(uname -m)-apple-macos13.1" \
  -parse-as-library \
  "$ROOT_DIR/macos/AppDataDirectory.swift" \
  "$ROOT_DIR/macos/AppOperationLogging.swift" \
  "$ROOT_DIR/macos/UserShellPathSupport.swift" \
  "$ROOT_DIR/macos/tests/UserShellPathTests.swift" \
  -o "$TEST_BINARY"

"$TEST_BINARY"
