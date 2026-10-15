# Antigravity provider

CLI/core owns model discovery, quota parsing and Gemini search translation.
macOS, Windows and the Web panel consume the same CLI responses.

## Models and refresh

The AGY preset starts without seed models. `providers discover <id>` reads the
signed-in official CLI's `models` JSON report. With an explicit provider key,
it queries the configured `v1internal:fetchAvailableModels` endpoint instead;
it never borrows the local CLI account for that key. Native authentication is
restricted to the supported Google HTTPS hosts.

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

`quota --provider <id> --json` reads the official CLI's read-only `/usage`
report. It returns a row for each measured group/window, including a stable
quota ID, label, `unit: percent`, remaining percentage and optional reset time.
Families and window names come from the report. Missing or invalid measurements
produce an explicit error rather than zero or full quota. Custom quota URLs
continue through the existing parser. Local CLI quota cannot represent an
explicit provider API key and returns an error for that configuration.

Native reports require `agy` 1.3.5 or later, the verified command baseline. Processes have bounded output,
timeouts and process-tree cleanup. They run from a temporary directory and
must return zero agent turns and zero usage. When file-based OAuth credentials
exist, reports use a private temporary copy of the same credentials as gateway
inference and force file storage instead of consulting an unrelated OS keyring.
Temporary credentials are removed after the report; the original is not rewritten.
Without a token file, reports use the signed-in CLI account. The existing HTTP
inference adapter still requires file-based OAuth credentials: a working quota
report does not establish that gateway inference authentication is configured.

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
[scoped file-token implementation](https://github.com/steipete/CodexBar/blob/main/Sources/CodexBarCore/Providers/Antigravity/AntigravityScopedPrintFetch.swift).

## Validation

`cargo test --locked --test agy_dynamic` covers configuration compatibility,
dynamic additions/removals, metadata updates, explicit-key discovery, CLI quota
rows and preservation after refresh failure. `cargo test --locked --test
gemini_regressions` covers search translation and grounding citations.

The ignored `live_agy_account_reports` test runs only read-only native reports
against the signed-in account. It is an opt-in integration check, not evidence
that real inference or search succeeded.
