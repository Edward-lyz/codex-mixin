#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MENU_BAR_FILE="$ROOT_DIR/macos/MenuBarApp.swift"
SERVICE_FILE="$ROOT_DIR/macos/GatewayService.swift"
SWITCH_FILE="$ROOT_DIR/macos/CodexGatewaySwitchSupport.swift"
LIFECYCLE_FILE="$ROOT_DIR/macos/GatewayLifecycleSupport.swift"

if rg -n 'applicationShouldTerminate' "$ROOT_DIR/macos" \
  --glob '*.swift' \
  --glob '!tests/**'
then
  echo "App termination must use AppKit's default lifecycle so the gateway keeps running" >&2
  exit 1
fi

if ! rg -q 'func applicationWillTerminate' "$MENU_BAR_FILE"; then
  echo "App termination must still clean up UI refresh resources" >&2
  exit 1
fi

if ! rg -q 'runGateway\(\["service", "stop", "--managed", "--json"\]\)' "$SERVICE_FILE"; then
  echo "The explicit gateway stop action must remain available" >&2
  exit 1
fi

if ! rg -q 'runGatewayStreaming\(\["codex-switch", "official"\]\)' "$SWITCH_FILE"; then
  echo "The macOS shell must delegate official-mode switching to the CLI" >&2
  exit 1
fi

if ! rg -q 'runGatewayStreaming\(\["codex-switch", "mixin"\]\)' "$SWITCH_FILE"; then
  echo "The macOS shell must delegate Mixin-mode switching to the CLI" >&2
  exit 1
fi

if rg -q 'uninstall-codex|service", "stop|config\.toml|UserDefaults' "$SWITCH_FILE"; then
  echo "The macOS shell must not own Codex config parsing, persistence, or gateway-stop policy" >&2
  exit 1
fi

if ! rg -q 'restartRunningCodexDesktopApp\(\)' "$SWITCH_FILE"; then
  echo "Managed Codex shutdown must restart the desktop app" >&2
  exit 1
fi

if rg -q 'ChatGPT\.app' "$SWITCH_FILE"; then
  echo "The Codex lifecycle must not restart the unrelated ChatGPT app" >&2
  exit 1
fi

if ! rg -q 'refreshCodexIntegrationStatus\(\)' "$LIFECYCLE_FILE" \
  || ! rg -q 'codexStatus\.isOfficialMode' "$LIFECYCLE_FILE"; then
  echo "App launch must use the CLI-reported Codex state before starting the gateway" >&2
  exit 1
fi

echo "Application exit keeps gateway running: passed"
