#!/usr/bin/env bash
# Validate the `.credentials.yaml` documents codex-mixin writes against DSH's
# own parser, rather than against this repo's transcription of DSH's rules.
#
# DSH upgrades the pre-release flat layout only in `loadInitial`; the watcher's
# `reconcileFromDisk` parses without migrating and keeps the last good snapshot
# when parsing fails. A document this script rejects would therefore be dropped
# silently by a running DSH, leaving the gateway Unauthorized until a restart.
#
# Skips when the DSH checkout is unavailable, so CI without it stays green.
# Point DSH_REPO at a DSH checkout to run it.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dsh_repo="${DSH_REPO:-${HOME}/deepseek-harness}"
parser="${dsh_repo}/packages/credentials/credentials-local/lib/index.js"

if [[ ! -f "${parser}" ]]; then
  echo "skip: DSH credentials parser not found at ${parser}"
  echo "      set DSH_REPO to a DSH checkout with packages built to run this check"
  exit 0
fi
if ! command -v node >/dev/null; then
  echo "skip: node is required to run DSH's parser"
  exit 0
fi

fixtures="${repo_root}/target/dsh-credential-fixtures"
cargo test --locked --bin codex-mixin emits_credential_fixtures_for_the_dsh_parser_e2e >/dev/null
if [[ ! -d "${fixtures}" ]]; then
  echo "FAIL: ${fixtures} was not produced" >&2
  exit 1
fi

node --input-type=module -e '
import { readdirSync, readFileSync } from "node:fs"
import { join } from "node:path"
const [parser, fixtures] = process.argv.slice(1)
const { parseCredentialsDocument } = await import(parser)
const files = readdirSync(fixtures).filter(name => name.endsWith(".yaml")).sort()
if (files.length === 0) throw new Error(`no fixtures in ${fixtures}`)
for (const name of files) {
  const path = join(fixtures, name)
  const { refs } = parseCredentialsDocument(readFileSync(path, "utf8"), path)
  const key = refs.get("CODEX_MIXIN_GATEWAY_API_KEY")
  if (key !== "fixture-key") {
    throw new Error(`${name}: DSH read CODEX_MIXIN_GATEWAY_API_KEY as ${JSON.stringify(key)}`)
  }
  console.log(`ok   ${name}: DSH loaded the gateway key`)
}
console.log(`DSH parser accepted ${files.length} document(s)`)
' "${parser}" "${fixtures}"
