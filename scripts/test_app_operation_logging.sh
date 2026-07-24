#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/app-operation-logging-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/AppOperationLogging.swift" \
  "$ROOT_DIR/macos/tests/AppOperationLoggingTests.swift" \
  -o "$TEST_BINARY"
"$TEST_BINARY"
