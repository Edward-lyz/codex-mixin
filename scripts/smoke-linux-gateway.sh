#!/usr/bin/env bash
# Starts a release binary against isolated state that already has a usage
# database, so the gateway opens bundled SQLite before it listens. A static
# binary can pass verify-linux-static.sh and still crash here when cached C
# objects were linked by an incompatible linker.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: smoke-linux-gateway.sh <binary>" >&2
  exit 2
fi

binary="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
if [[ ! -x "$binary" ]]; then
  echo "missing executable Linux binary: $binary" >&2
  exit 1
fi

smoke_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-mixin-smoke.XXXXXX")"
# Isolate every state location and the OS startup-service identity so the
# smoke run can never reach a real gateway or service on the runner.
export HOME="$smoke_dir/home"
export DSH_HOME="$smoke_dir/home/.dsh"
export CODEX_HOME="$smoke_dir/codex-home"
export CODEX_GATEWAY_CONFIG="$smoke_dir/config.json"
export CODEX_GATEWAY_RUNTIME_FILE="$smoke_dir/runtime.json"
export CODEX_GATEWAY_DAEMON_FILE="$smoke_dir/daemon.json"
export CODEX_GATEWAY_SERVICE_ID="smoke-$$"
export XDG_CONFIG_HOME="$smoke_dir/xdg-config"
export XDG_DATA_HOME="$smoke_dir/xdg-data"
export XDG_STATE_HOME="$smoke_dir/xdg-state"
# Startup must not depend on the network; refuse every outbound request.
export HTTP_PROXY="http://127.0.0.1:9" HTTPS_PROXY="http://127.0.0.1:9"
export http_proxy="$HTTP_PROXY" https_proxy="$HTTPS_PROXY"
export NO_PROXY="localhost,127.0.0.1" no_proxy="localhost,127.0.0.1"
mkdir -p "$HOME" "$CODEX_HOME"
gateway_log="$smoke_dir/gateway.log"
gateway_pid=""

cleanup() {
  if [[ -n "$gateway_pid" ]]; then
    kill "$gateway_pid" 2>/dev/null || true
    wait "$gateway_pid" 2>/dev/null || true
  fi
  rm -rf -- "$smoke_dir"
}
trap cleanup EXIT

"$binary" --no-tui providers add \
  --preset custom --id smoke --key smoke-key \
  --base-url "http://127.0.0.1:9" --model smoke-model >"$smoke_dir/provider.log" 2>&1 || {
  echo "provider setup failed:" >&2
  cat "$smoke_dir/provider.log" >&2
  exit 1
}
# The gateway opens usage.sqlite3 at startup only when it exists. An empty
# file is a valid empty SQLite database.
: >"$(dirname "$CODEX_GATEWAY_CONFIG")/usage.sqlite3"

"$binary" --no-tui start --bind 127.0.0.1:0 >"$gateway_log" 2>&1 &
gateway_pid=$!
for _ in {1..200}; do
  if grep -q 'codex-mixin listening' "$gateway_log"; then
    echo "gateway smoke test: listening with an existing usage database"
    exit 0
  fi
  if ! kill -0 "$gateway_pid" 2>/dev/null; then
    status=0
    wait "$gateway_pid" || status=$?
    gateway_pid=""
    echo "gateway exited before listening (status $status):" >&2
    cat "$gateway_log" >&2
    exit 1
  fi
  sleep 0.1
done
echo "gateway did not start listening within 20 seconds:" >&2
cat "$gateway_log" >&2
exit 1
