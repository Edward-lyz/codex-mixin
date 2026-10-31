#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$ROOT_DIR/tests/client_auth_tests.txt"
cd "$ROOT_DIR"

passed=0
while IFS='|' read -r scope source test_name extra; do
  if [[ -z "$scope" || "$scope" == \#* ]]; then
    continue
  fi
  if [[ -z "$source" || -z "$test_name" || -n "$extra" ]]; then
    echo "invalid client-auth test manifest row: $scope|$source|$test_name|$extra" >&2
    exit 2
  fi

  args=(test --locked --features client-auth)
  case "$scope" in
    cors | gateway_http | gateway_startup)
      args+=(--test "$scope" "$test_name")
      ;;
    *)
      echo "unknown client-auth test scope: $scope" >&2
      exit 2
      ;;
  esac
  args+=(-- --exact)

  output="$(cargo "${args[@]}" 2>&1)" || {
    printf '%s\n' "$output"
    exit 1
  }
  if ! grep -Eq 'test result: ok\. 1 passed; 0 failed;' <<<"$output"; then
    printf '%s\n' "$output"
    echo "client-auth test did not run exactly once: $test_name" >&2
    exit 1
  fi
  passed=$((passed + 1))
done < "$MANIFEST"

echo "client-auth targeted tests passed: $passed"
