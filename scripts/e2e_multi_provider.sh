#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$repo_root/target/debug/codex-mixin"
if [[ ! -x "$binary" ]]; then
  cargo build --manifest-path "$repo_root/Cargo.toml"
fi

e2e_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-mixin-e2e.XXXXXX")"
alpha_ready="$e2e_dir/alpha.port"
beta_ready="$e2e_dir/beta.port"
alpha_log="$e2e_dir/alpha.ndjson"
beta_log="$e2e_dir/beta.ndjson"
gateway_log="$e2e_dir/gateway.log"
gateway_pid=""
alpha_pid=""
beta_pid=""

cleanup() {
  for pid in "$gateway_pid" "$alpha_pid" "$beta_pid"; do
    if [[ -n "$pid" ]]; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
}
trap cleanup EXIT

node "$repo_root/scripts/e2e_mock_provider.mjs" \
  anthropic alpha-secret "$alpha_log" "$alpha_ready" 10 >"$e2e_dir/alpha.log" 2>&1 &
alpha_pid=$!
node "$repo_root/scripts/e2e_mock_provider.mjs" \
  openai beta-secret "$beta_log" "$beta_ready" 20 >"$e2e_dir/beta.log" 2>&1 &
beta_pid=$!
for _ in {1..100}; do
  [[ -s "$alpha_ready" && -s "$beta_ready" ]] && break
  sleep 0.05
done
[[ -s "$alpha_ready" && -s "$beta_ready" ]]
alpha_port="$(<"$alpha_ready")"
beta_port="$(<"$beta_ready")"

export HOME="$e2e_dir/home"
export CODEX_HOME="$e2e_dir/codex-home"
export CODEX_GATEWAY_CONFIG="$e2e_dir/config.json"
mkdir -p "$HOME" "$CODEX_HOME"

"$binary" providers add \
  --preset custom --id alpha --key alpha-secret \
  --base-url "http://127.0.0.1:$alpha_port" \
  --protocol anthropic_messages --api-path /v1/messages \
  --quota-url "http://127.0.0.1:$alpha_port/quota" \
  --quota-currency CNY --quota-parser generic \
  --model shared --model hidden --model broken
"$binary" providers select alpha --model shared --model broken
"$binary" providers add \
  --preset custom --id beta --key beta-secret \
  --base-url "http://127.0.0.1:$beta_port" \
  --protocol open_ai_chat --api-path /v1/chat/completions \
  --quota-url "http://127.0.0.1:$beta_port/quota" \
  --quota-currency USD --quota-parser generic \
  --model shared
if "$binary" providers add \
  --preset baidu-oneapi --id missing-quota-user --key baidu-secret \
  >"$e2e_dir/baidu-missing-username.log" 2>&1; then
  echo "Baidu OneAPI unexpectedly accepted a missing quota username" >&2
  exit 1
fi
grep -F "requires a quota username" "$e2e_dir/baidu-missing-username.log" >/dev/null

gateway_port="$(
  node -e 'const n=require("node:net").createServer();n.listen(0,"127.0.0.1",()=>{console.log(n.address().port);n.close()})'
)"
"$binary" serve --bind "127.0.0.1:$gateway_port" >"$gateway_log" 2>&1 &
gateway_pid=$!
gateway_url="http://127.0.0.1:$gateway_port"
for _ in {1..100}; do
  curl -fsS "$gateway_url/healthz" >/dev/null 2>&1 && break
  sleep 0.05
done
curl -fsS "$gateway_url/healthz" >/dev/null

curl -fsS "$gateway_url/v1/models" >"$e2e_dir/models.json"
jq -e '
  [.data[].id] == ["shared-alpha", "broken-alpha", "shared-beta"] and
  ([.data[].id] | index("hidden-alpha")) == null
' "$e2e_dir/models.json" >/dev/null

response_request='{"model":"MODEL","stream":true,"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}]}'
alpha_status="$(
  curl -sS -o "$e2e_dir/alpha-response.sse" -w '%{http_code}' \
    -H 'content-type: application/json' \
    --data "${response_request/MODEL/shared-alpha}" \
    "$gateway_url/v1/responses"
)"
beta_status="$(
  curl -sS -o "$e2e_dir/beta-response.sse" -w '%{http_code}' \
    -H 'content-type: application/json' \
    --data "${response_request/MODEL/shared-beta}" \
    "$gateway_url/v1/responses"
)"
hidden_status="$(
  curl -sS -o "$e2e_dir/hidden-response.json" -w '%{http_code}' \
    -H 'content-type: application/json' \
    --data "${response_request/MODEL/hidden-alpha}" \
    "$gateway_url/v1/responses"
)"
broken_status="$(
  curl -sS -o "$e2e_dir/broken-response.json" -w '%{http_code}' \
    -H 'content-type: application/json' \
    --data "${response_request/MODEL/broken-alpha}" \
    "$gateway_url/v1/responses"
)"
beta_after_failure_status="$(
  curl -sS -o "$e2e_dir/beta-after-failure.sse" -w '%{http_code}' \
    -H 'content-type: application/json' \
    --data "${response_request/MODEL/shared-beta}" \
    "$gateway_url/v1/responses"
)"
[[ "$alpha_status" == 200 ]]
[[ "$beta_status" == 200 ]]
[[ "$hidden_status" == 400 ]]
[[ "$broken_status" == 502 ]]
[[ "$beta_after_failure_status" == 200 ]]

node "$repo_root/scripts/e2e_ws_client.mjs" \
  "ws://127.0.0.1:$gateway_port/v1/responses" >"$e2e_dir/websocket.json"

curl -fsS -H 'content-type: application/json' \
  --data '{"timeout_seconds":5,"providers":["alpha","beta"],"models":["shared-alpha","shared-beta"]}' \
  "$gateway_url/v1/model-benchmarks" >"$e2e_dir/benchmark-start.json"
for _ in {1..200}; do
  curl -fsS "$gateway_url/v1/model-benchmarks" >"$e2e_dir/benchmark.json"
  [[ "$(jq -r '.snapshot.status' "$e2e_dir/benchmark.json")" == completed ]] && break
  sleep 0.05
done
jq -e '
  .snapshot.status == "completed" and
  ([.snapshot.results[].provider_id] | sort) == ["alpha", "beta"] and
  ([.snapshot.provider_costs[].currency] | sort) == ["CNY", "USD"]
' "$e2e_dir/benchmark.json" >/dev/null

"$binary" quota --json >"$e2e_dir/quota.json"
jq -e '
  map({key: .provider_id, value: .currency}) | from_entries ==
  {"alpha":"CNY","beta":"USD"}
' "$e2e_dir/quota.json" >/dev/null
"$binary" status --json >"$e2e_dir/status.json"
jq -e '
  .gateway == "running" and
  .provider_readiness == "healthy" and
  .provider_counts == {"total":2,"healthy":2,"degraded":0,"disabled":0} and
  ([.providers[].readiness.status] | unique) == ["healthy"]
' "$e2e_dir/status.json" >/dev/null
"$binary" doctor --json >"$e2e_dir/doctor.json"
jq -e '
  .ok == true and
  .summary.errors == 0 and
  ([.providers[].provider_id] | sort) == ["alpha", "beta"] and
  ([.providers[].paid_inference_performed] | unique) == [false]
' "$e2e_dir/doctor.json" >/dev/null
CODEX_GATEWAY_BIND="127.0.0.1:1" \
CODEX_GATEWAY_KEY="ignored-gateway-key" \
CODEX_GATEWAY_THINKING_MODE="invalid" \
CODEX_GATEWAY_OFFICIAL_RESPONSES_URL="http://127.0.0.1:2/responses" \
  "$binary" config --json --scope effective >"$e2e_dir/config-env-ignored.json"
jq -e --arg endpoint "$gateway_url" '
  .bind == ($endpoint | sub("^http://"; "")) and
  .gateway_api_key == null and
  .official_responses_url == "https://chatgpt.com/backend-api/codex/responses" and
  .thinking_mode == "Auto"
' "$e2e_dir/config-env-ignored.json" >/dev/null
"$binary" config --json --scope stored >"$e2e_dir/config-redacted.json"
curl -fsS "$gateway_url/v1/codex-model-catalog" >"$e2e_dir/catalog.json"
if grep -F -e alpha-secret -e beta-secret \
  "$e2e_dir/config-redacted.json" "$e2e_dir/catalog.json" \
  "$e2e_dir/benchmark.json" "$gateway_log"; then
  echo "secret leaked into a redacted output" >&2
  exit 1
fi
[[ "$(stat -f '%Lp' "$CODEX_GATEWAY_CONFIG")" == 600 ]]
grep -F "gateway configuration loaded from stored config" "$gateway_log" >/dev/null
grep -F "provider_id=alpha" "$gateway_log" >/dev/null
grep -F "catalog_slug=shared-alpha" "$gateway_log" >/dev/null
grep -F "gateway upstream request failed" "$gateway_log" >/dev/null

jq -s -e '
  any(.[]; .authorization == "Bearer alpha-secret" and .body.model == "shared") and
  any(.[]; .authorization == "Bearer alpha-secret" and .body.model == "broken")
' "$alpha_log" >/dev/null
jq -s -e '
  all(.[]; .authorization == "Bearer beta-secret" and .body.model == "shared")
' "$beta_log" >/dev/null

if "$binary" providers update alpha --clear-key >"$e2e_dir/clear-enabled.log" 2>&1; then
  echo "enabled provider unexpectedly allowed API key clearing" >&2
  exit 1
fi
"$binary" providers disable beta
"$binary" providers update beta --clear-key
"$binary" providers list --json >"$e2e_dir/providers-after-clear.json"
jq -e '
  (.providers[] | select(.id == "alpha") | .api_key_configured) == true and
  (.providers[] | select(.id == "beta") |
    .api_key_configured == false and .readiness == "disabled")
' "$e2e_dir/providers-after-clear.json" >/dev/null

echo "multi-provider E2E passed"
echo "artifacts: $e2e_dir"
