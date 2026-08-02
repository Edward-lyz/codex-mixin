#!/usr/bin/env bash
# End-to-end guard for the provider prompt-prefix cache contract.
#
# Drives two turns through a real gateway against mock Anthropic Messages and
# OpenAI Chat providers, then asserts the upstream bytes: the earlier prompt is
# replayed unchanged, fresh tool images are inlined once, replayed ones collapse
# to a stable marker, and Chat never receives an image inside a `tool` message.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$repo_root/target/debug/codex-mixin"
if [[ ! -x "$binary" ]]; then
  cargo build --manifest-path "$repo_root/Cargo.toml"
fi

e2e_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-mixin-prompt-cache.XXXXXX")"
anthropic_ready="$e2e_dir/anthropic.port"
chat_ready="$e2e_dir/chat.port"
gateway_log="$e2e_dir/gateway.log"
gateway_pid=""
anthropic_pid=""
chat_pid=""

cleanup() {
  for pid in "$gateway_pid" "$anthropic_pid" "$chat_pid"; do
    if [[ -n "$pid" ]]; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
}
trap cleanup EXIT

node "$repo_root/scripts/e2e_mock_provider.mjs" \
  anthropic alpha-secret "$e2e_dir/anthropic.ndjson" "$anthropic_ready" 10 \
  >"$e2e_dir/anthropic.stdout" 2>&1 &
anthropic_pid=$!
node "$repo_root/scripts/e2e_mock_provider.mjs" \
  openai beta-secret "$e2e_dir/chat.ndjson" "$chat_ready" 20 \
  >"$e2e_dir/chat.stdout" 2>&1 &
chat_pid=$!
for _ in {1..100}; do
  [[ -s "$anthropic_ready" && -s "$chat_ready" ]] && break
  sleep 0.05
done
[[ -s "$anthropic_ready" && -s "$chat_ready" ]]
anthropic_port="$(<"$anthropic_ready")"
chat_port="$(<"$chat_ready")"

node "$repo_root/scripts/e2e_prompt_cache.mjs" build "$e2e_dir"

export HOME="$e2e_dir/home"
export CODEX_HOME="$e2e_dir/codex-home"
export CODEX_GATEWAY_CONFIG="$e2e_dir/config.json"
mkdir -p "$HOME" "$CODEX_HOME"

"$binary" providers add \
  --preset custom --id alpha --key alpha-secret \
  --base-url "http://127.0.0.1:$anthropic_port" \
  --protocol anthropic_messages --api-path /v1/messages \
  --model shared >/dev/null
"$binary" providers add \
  --preset custom --id beta --key beta-secret \
  --base-url "http://127.0.0.1:$chat_port" \
  --protocol open_ai_chat --api-path /v1/chat/completions \
  --model shared >/dev/null

gateway_port="$(
  node -e 'const n=require("node:net").createServer();n.listen(0,"127.0.0.1",()=>{console.log(n.address().port);n.close()})'
)"
# Debug logging is what makes the per-turn prefix diagnostics observable.
RUST_LOG=codex_mixin=debug "$binary" serve --bind "127.0.0.1:$gateway_port" \
  >"$gateway_log" 2>&1 &
gateway_pid=$!
gateway_url="http://127.0.0.1:$gateway_port"
for _ in {1..200}; do
  curl -fsS "$gateway_url/healthz" >/dev/null 2>&1 && break
  sleep 0.05
done
curl -fsS "$gateway_url/healthz" >/dev/null

post_turn() {
  local payload="$e2e_dir/$1.json" out="$e2e_dir/$1.sse" status
  status="$(
    curl -sS -o "$out" -w '%{http_code}' \
      -H 'content-type: application/json' \
      --data-binary "@$payload" \
      "$gateway_url/v1/responses"
  )"
  if [[ "$status" != 200 ]]; then
    echo "$1 failed with HTTP $status" >&2
    cat "$out" >&2
    exit 1
  fi
  grep -F 'response.completed' "$out" >/dev/null
}

post_turn anthropic-turn1
post_turn anthropic-turn2
post_turn chat-turn1
post_turn chat-turn2

node "$repo_root/scripts/e2e_prompt_cache.mjs" verify "$e2e_dir"
echo "artifacts: $e2e_dir"
