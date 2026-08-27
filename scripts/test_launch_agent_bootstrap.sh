#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/launch-agent-bootstrap-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/LaunchAgentBootstrapSupport.swift" \
  "$ROOT_DIR/macos/tests/LaunchAgentBootstrapTests.swift" \
  -o "$TEST_BINARY"
"$TEST_BINARY"
