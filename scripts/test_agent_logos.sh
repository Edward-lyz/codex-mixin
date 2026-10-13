#!/bin/bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
# Validate the packaged files the actual app will load, including PNG assets.
xcrun swift "$ROOT_DIR/macos/tests/AgentLogoTests.swift" \
  "$ROOT_DIR/dist/Codex Mixin.app/Contents/Resources/AgentLogos"
