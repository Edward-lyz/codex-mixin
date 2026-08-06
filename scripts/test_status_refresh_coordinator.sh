#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/status-refresh-coordinator-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/StatusRefreshCoordinator.swift" \
  "$ROOT_DIR/macos/tests/StatusRefreshCoordinatorTests.swift" \
  -o "$TEST_BINARY"
"$TEST_BINARY"
