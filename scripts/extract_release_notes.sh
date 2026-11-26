#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: extract_release_notes.sh <tag> <out-file>" >&2
  exit 2
fi

tag="$1"
out_file="$2"
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
notes_file="$root_dir/RELEASE_NOTES.md"

# Publish only the current version. The zh-Hans block holds every
# version, so select the section headed by the release tag and stop
# at the next version heading.
awk -v heading="## $tag" '
  /^<!-- codex-mixin:[A-Za-z-]+:start -->$/ {
    locale = $0
    sub(/^<!-- codex-mixin:/, "", locale)
    sub(/:start -->$/, "", locale)
    next
  }
  /^<!-- codex-mixin:[A-Za-z-]+:end -->$/ { locale = ""; emit = 0; next }
  locale == "zh-Hans" && $0 == heading { emit = 1 }
  locale == "zh-Hans" && emit && /^## / && $0 != heading { emit = 0 }
  locale == "zh-Hans" && emit { print }
' "$notes_file" > "$out_file"

source_count="$(grep -Fxc "## $tag" "$notes_file" || true)"
if [[ "$source_count" != "3" ]]; then
  echo "expected three localized headings for $tag in RELEASE_NOTES.md, found ${source_count:-0}" >&2
  exit 1
fi
output_count="$(grep -Fxc "## $tag" "$out_file" || true)"
version_count="$(grep -Ec '^## v' "$out_file" || true)"
if [[ "$output_count" != "1" || "$version_count" != "1" ]]; then
  echo "expected only the $tag section in the release notes, found ${output_count:-0} tag headings and ${version_count:-0} version headings" >&2
  exit 1
fi
