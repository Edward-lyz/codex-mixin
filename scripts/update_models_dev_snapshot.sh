#!/usr/bin/env bash
# Refreshes the models.dev catalog snapshot bundled into the binary. Release
# builds run this first so every package ships the latest catalog; offline
# installs use it until a local models.dev cache exists. Requires curl, jq
# and zstd.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT="$ROOT_DIR/src/provider/models_dev_api.json.zst"
# Staged beside the output so the final rename is atomic.
STAGED="$(mktemp "$(dirname "$OUTPUT")/.models_dev_api.XXXXXX")"
trap 'rm -f "$STAGED"' EXIT

curl -fsS --retry 3 --max-time 60 https://models.dev/api.json \
  | jq -c 'if type == "object"
      and all(.[]; type == "object" and (.models | type) == "object")
      and ([.[].models | length] | add // 0) > 0
    then . else error("unexpected models.dev catalog shape") end' \
  | zstd -19 -q -c > "$STAGED"
chmod 644 "$STAGED"
mv "$STAGED" "$OUTPUT"
trap - EXIT
echo "$OUTPUT"
