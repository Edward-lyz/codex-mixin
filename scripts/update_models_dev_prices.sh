#!/usr/bin/env bash
# Refreshes the bundled models.dev price snapshot used before the runtime
# models.dev cache exists. Requires curl, jq and gzip.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT="$ROOT_DIR/src/provider/models_dev_prices.json.gz"
# Staged beside the output so the final rename is atomic.
STAGED="$(mktemp "$(dirname "$OUTPUT")/.models_dev_prices.XXXXXX")"
trap 'rm -f "$STAGED"' EXIT

curl -fsS --max-time 60 https://models.dev/api.json \
  | jq -cS '
      with_entries(.value = {
        models: ((.value.models // {})
          | with_entries(select(.value.cost.input != null and .value.cost.output != null)
            | .value = ({
                cost: (.value.cost | {input, output, cache_read, cache_write, tiers, context_over_200k}
                  | with_entries(select(.value != null))),
                canonical_model_id: .value.canonical_model_id
              } | with_entries(select(.value != null)))))
      })
      | with_entries(select(.value.models | length > 0))' \
  | gzip -9n > "$STAGED"
chmod 644 "$STAGED"
mv "$STAGED" "$OUTPUT"
trap - EXIT
echo "$OUTPUT"
