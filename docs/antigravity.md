# Antigravity provider

CLI/core owns model discovery, quota parsing and Gemini search translation.
macOS, Windows and the Web panel consume the same CLI responses.

## Models and refresh

The AGY preset starts without seed models. `providers discover <id>` directly
calls `v1internal:fetchAvailableModels`. OAuth authentication, token renewal,
project discovery and inference run in Rust; no AGY executable is required.
For native OAuth, `loadCodeAssist` resolves the account's Cloud Code project,
which is cached by endpoint and credential and shared with inference/probes.
Explicit provider keys never borrow the local OAuth account.

The running gateway refreshes enabled AGY providers every 15 minutes. Failures
back off to that interval and retain the last successful catalog, timestamp
and selection, with `models_refresh_error` describing the failure. A valid empty
catalog removes routable models. Missing selected IDs remain selected so their
selection returns if the upstream ID reappears. Subsequent additions are shown
as new models and are not automatically selected. Context overrides survive
model removal. Unknown capabilities and token limits remain unknown.

For compatibility with installed CLIs, configuration continues to store
`model_source.kind = static`. Core dispatches dynamic discovery using the AGY
preset ID. It also reads the transient development tag `antigravity`, but never
writes it. Loading configuration does not reset the refresh timestamp.
Older binaries can parse the configuration but retain their older behavior.

## Quota and credentials

`quota --provider <id> --json` directly calls
`v1internal:retrieveUserQuotaSummary`. It returns one row per measured
bucket, including upstream bucket ID, label, `unit: percent`, remaining
percentage and optional reset time. Names and windows come from the response.
Missing or invalid measurements produce an error rather than zero/full quota.
Explicit provider keys use their own credentials for quota as well as models.
Custom quota URLs continue through the existing parser.

Choose AGY in Add Provider or run `codex-mixin --no-tui providers add --preset agy`.
Browser login uses PKCE, a random state and a loopback callback. It defaults
to 120 seconds and supports `--oauth-timeout-seconds` (1–300). Cancel in the
form or use Ctrl-C in the CLI to stop waiting. Tokens are stored privately in Mixin's
OAuth directory, next to its configuration, and renewed without running a CLI.
Legacy OAuth files can still be read. A newly signed-in native account clears
old native AGY model directories while preserving selection intent; explicit-key
providers keep their independent directory. Each native request is restricted
to the supported Google HTTPS hosts. HTTP output and request duration are bounded.

AGY catalog refresh and model selection do not automatically run capability
probes. Explicit `providers probe` requests can consume quota. Native account
probe results are not reused across sessions because reports do not expose a
stable account identifier. Explicit-key probe caches include the key fingerprint.

## Search

For Gemini models, Responses `web_search` and `web_search_preview` requests are
translated to Gemini `googleSearch`, including requests with function tools.
Grounding sources become Responses URL citation annotations and completed
`web_search_call` output. UTF-8 grounding byte offsets become character offsets.
Unsupported search options fail explicitly rather than silently losing filters.
A search probe only reports support when the response contains search grounding;
a successful HTTP status alone is inconclusive.

This adapter covers Gemini native Google Search. AGY SDK `SEARCH_WEB` and
`READ_URL_CONTENT` are agent tools; forwarding them for Claude or GPT requires
an SDK execution adapter and is not implemented by the Gemini HTTP converter.
Those models are not marked as supporting Google Search.

References: [official CLI changelog](https://antigravity.google/docs/changelog?tab=cli),
[official SDK tools](https://antigravity.google/docs/sdk/tools),
[Gemini Google Search](https://ai.google.dev/gemini-api/docs/google-search/),
[DSH native OAuth adapter](https://github.com/LiZhenNet/dsh-antigravity/blob/main/lib/index.js).

## Validation

`cargo test --locked --test agy_dynamic` covers configuration compatibility,
dynamic additions/removals, metadata updates, explicit-key discovery, CLI quota
rows and preservation after refresh failure. `cargo test --locked --test
gemini_regressions` covers search translation and grounding citations.

The ignored `live_agy_account_reports` test directly queries read-only HTTP
endpoints with saved OAuth credentials. It is opt-in and does not establish
that real inference or search succeeded.
