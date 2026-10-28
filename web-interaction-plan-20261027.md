# Web interaction repair plan — 2026-10-27

## Scope and planning state

User requests responsive Baidu provider saving, browser-visible DUCX QR/login output, and inspection of related frontend/backend stalls. Planning precedes implementation. The current session has no tool to switch the application's collaboration mode; this file records the planning phase explicitly. The user approved the necessary functions, streaming protocol and piped-login mode; implementation and verification are complete.

Repository: codex-mixin, initially clean main at bf312e4. The earlier thread installed Linux v0.7.5; this checkout currently declares 0.7.4, so compare release ancestry before implementing.

Ancestry checked: v0.7.5 is db0f368 and main adds db218ce and bf312e4. The inspected panel/cli.rs, panel/api.rs and src/cli/ducx_setup.rs have no differences from v0.7.5. The existing target/debug/codex-mixin reports 0.7.5.

## Current behavior established by source inspection

- panel/cli.rs uses Command::output(), collecting stdout/stderr until exit, with a 600-second timeout and null stdin. Existing MIXIN_PROGRESS messages are not delivered live.
- panel/api.rs serializes actions and returns one final JSON response. Individual CLI steps overwrite stdout. Cancellation has unknown commit state.
- src/cli/providers/management.rs commits a provider before discovery and capability probing finish. A waiting browser therefore does not indicate whether the configuration was already saved.
- src/cli/ducx_setup.rs downloads/installs DUCX before login, then opens a host terminal when stdin is not a terminal. A headless remote Web user cannot see that terminal.
- panel/assets/app.js awaits a full snapshot refresh inside runAction's finally block; action completion is coupled to unrelated status queries. Review pending modal close/cancel behavior and error handling as part of reproduction.

Runtime reproduction confirmed buffered save feedback; isolated real DUCX emitted QR text through pipes. Browser testing confirmed completion remains responsive when snapshot refresh is stalled.

## Proposed files and changes

1. panel/tests.rs and/or tests/panel_interaction.rs: deterministic real CLI/Router reproduction using isolated state and controlled slow providers/login fixture; assert time to first progress, persisted state, cancellation, authentication and completion semantics. Use tests/common for real binary launches.
2. panel/cli.rs, panel/api.rs: extend the existing action path using Tokio process pipes and Axum streaming responses; deliver bounded stdout/stderr progress and a terminal result. Preserve existing JSON clients and structured committed/stage errors. Keep action serialization and request authentication. No general job framework.
3. src/cli/args.rs, src/cli/ducx_setup.rs (and existing CLI dispatch if required): explicit non-terminal login output contract for the Web caller, forwarding DUCX login output through existing subprocess streams rather than launching a host GUI terminal. Verify whether DUCX login supports pipes; if it requires PTY, first evaluate existing platform APIs and seek approval for any new adapter/dependency.
4. src/cli/providers/management.rs: expose the existing save/discovery/probe phases through the existing progress mechanism; measure unnecessary synchronous work before changing save semantics.
5. panel/assets/app.js, panel/assets/index.html, panel/assets/app.css: native dialog showing immediate activity, streaming command output and legible QR, completion/failure and cancel controls. Consume streams using native fetch/ReadableStream/TextDecoder. Bound rendered history, use textContent, preserve CSP, keyboard access, narrow-screen layout and themes. Separate successful mutation feedback from refresh failures and prevent duplicate submissions.
6. Existing interface documentation/tests: document only the approved streaming and non-terminal login contract changes. Update affected directory instructions only if needed to accurately describe the contract.

## Approval boundary

User's AGENTS.md requires approval before hand-written functions/abstractions; repository AGENTS.md also requires approval for new protocol fields/APIs. The proposed streaming action contract, browser event handling and explicit piped-login mode need that approval. Prefer native APIs and edits to existing functions; do not introduce generic abstraction layers.

## Expected behavior

- Saving opens progress immediately; the user sees configuration saved separately from model discovery/probing, and receives a truthful final state.
- DUCX download and QR/login output appear in the browser dialog on headless hosts. Closing/cancelling has explicit semantics and does not falsely claim rollback.
- Failed refreshes do not turn an already successful action into apparent failure or leave controls indefinitely disabled.

## Risks and validation

- Login output may contain ANSI cursor controls or require a TTY; verify actual DUCX behavior before choosing rendering or transport.
- Stream disconnect/timeout must terminate owned processes and release the mutation lock; do not leave background login children or promise rollback after commit.
- Logs may contain credentials: retain existing secret transport and introduce explicit bounded/safe output handling, never echo submitted keys.
- Preserve existing CLI JSON/error consumers and desktop terminal behavior.
- Run focused red/green repro, then fmt, strict Clippy, architecture checks and full Rust tests per AGENTS.md. Check the browser in light/dark themes and narrow widths. Report external login steps that cannot be completed unattended separately.
- Clean only task-owned temporary fixtures/logs/build artifacts. Do not erase unrelated working state or active caches.
- Commit the validated minimal change locally; no push/release or remote reinstall without explicit instruction.

## Reproduction evidence (before implementation)

- Ran an inline `rtk proxy python3` harness against the existing real v0.7.5 binary. It launched `panel --bind 127.0.0.1:0` with temporary HOME/CODEX_HOME/config/runtime/XDG paths, isolated service ID, and dead external proxies. A loopback TCP listener accepted the Baidu model discovery request without replying.
- Sent authenticated `POST /api/actions` with `provider_add`, preset `baidu-oneapi`, dummy credentials, and the controlled loopback base URL. Independently queried `providers list --json` from the same isolated configuration.
- Output: `{"elapsed_seconds": 3.14, "provider_persisted": true, "pending_upstream_connections": 1, "http_response_bytes": 0, "reproduced_saved_but_no_feedback": true}`.
- This exercises the reported saved-but-invisible state through the real HTTP/CLI/persistence path; no real credentials or external model requests were used. The temporary directory and process group were cleaned up in finally.
- Ranked hypotheses for the next phase: (1) buffered subprocess output hides successful commit and subsequent discovery; (2) headless DUCX login chooses a host terminal instead of forwarding output; (3) runAction waits for unrelated snapshot work after action completion. Only the first currently has runtime reproduction evidence.
- Protocol/function approval received: implement and verify as planned. No additional dependencies, PTY adapter, provider persistence changes or CLI flags were needed; an explicit environment mode supplies pipe login.

## Implemented result and verification

- Added authenticated NDJSON action streaming and a native browser progress/QR dialog; retained the legacy JSON action endpoint. Cancellation/disconnect terminates the isolated CLI process tree and releases serialization.
- Reused existing provider-added progress rather than changing provider commit/discovery semantics. DUCX pipe login uses `CODEX_MIXIN_LOGIN_STDIO=1`, keeps QR output on stderr and advertises `login_stdio` through the interface.
- Detached completion from snapshot refresh, bounded GET waits and ignored stale snapshot responses.
- Initial three transport regression tests failed with 404 before implementation and passed afterward. Expanded coverage checks committed discovery failures, nested login cleanup on disconnect, JSON stdout and split/escaped secret redaction.
- Real installed DUCX, isolated with null stdin: 3417 stdout bytes within 8 seconds, QR block characters present, no ANSI; terminated without authenticating a real account.
- Chrome exercised the real isolated panel and controlled login fixture in light/dark themes and at 375 x 812. Completion and Close remained usable with a deliberately stalled snapshot. No page errors. Native Vision decoded both desktop and narrow-screen screenshots to the expected nonsecret test URL.
- Full all-target suites passed: default 1120 tests, client-auth 1121 tests, zero failures. Strict Clippy passed for both feature configurations. After the final terminal-event stream closure adjustment, default strict Clippy and 221 binary tests plus all 5 panel integration tests passed again. Formatting, JavaScript syntax, diff whitespace and both architecture checks passed.
- No version bump, push, release or remote deployment is included.

## Delivery — 2026-10-28

- Task-owned temporary harnesses, logs and screenshots are removed after recording their results here. Existing shared build caches are retained.
- The gateway prompt-cache E2E script was not run: this change affects panel action transport and CLI login, without changing the gateway request or cache path.
- Real account approval and Linux deployment were not performed; the browser login completion test uses an isolated fixture.
- Delivery is a local commit only.
