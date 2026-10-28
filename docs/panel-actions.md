# Web action progress and headless login

The panel still executes the real CLI. `POST /api/actions` retains its final JSON response for existing consumers. The Web interface uses authenticated `POST /api/actions/stream` with the same allowlisted action body and bearer token.

The response is `application/x-ndjson`, with one JSON object per line:

| Type | Fields | Meaning |
| --- | --- | --- |
| `step` | `index`, `total` | One-based position in a multi-command action. |
| `output` | `channel`, `text` | Incremental stdout/stderr text; terminal controls are not HTML. |
| `heartbeat` | none | Keep the connection active during quiet work. |
| `result` | `ok`, `stdout`, `json` | Successful completion; stdout/json describe the final CLI command. |
| `error` | `error.code`, `error.message`, `error.stage`, `error.committed` | Failed or cancelled completion, preserving the CLI error contract. |

Validation/authentication/conflict failures use ordinary HTTP error responses before streaming starts. Once streaming starts, callers must inspect the terminal record; HTTP 200 alone does not imply success. EOF without a terminal record means the result is unknown. Progress from all commands is streamed even when an action has several steps.

Actions remain serialized. `POST /api/actions/cancel`, a disconnected stream, or a 600-second per-command timeout terminates the owned CLI process tree, including nested login processes, before releasing the action lock. Cancellation cannot undo committed configuration. In particular, provider saving precedes discovery/probing; a later failure may carry `committed: true` and a stage such as `model discovery`. Cancellation and transport failures retain unknown commit state (`null`).

Output is bounded to 64 KiB per line and 1 MiB per pipe per command, with a bounded event queue. Exceeding a limit produces `output_limit` and stops the command. Submitted secrets are redacted before output is forwarded. The browser retains the latest 65,536 characters and renders them as text. Refresh requests have a separate 15-second timeout; failed or stale refreshes do not delay action completion or overwrite newer snapshots.

## DUCX pipe login

`interface --json` advertises `capabilities.login_stdio: true`. From a shell with Mixin installed, callers that can display login output can request:

```sh
CODEX_MIXIN_LOGIN_STDIO=1 codex-mixin --no-tui --json-errors connect ducx --json
```

The panel sets this environment variable on its CLI child automatically. DUCX download/progress and QR login output are forwarded to stderr, while successful JSON stdout stays parseable. No host terminal is opened in this mode. The browser displays the QR in a native dialog, with a white background in both themes. The user still needs to scan and approve the login. Existing terminal/desktop login behavior remains the default when the variable is absent.

Regression coverage: `cargo test --locked --test panel_interaction` exercises slow discovery after save, authentication, cancellation, disconnect cleanup, committed errors and headless login through real isolated CLI processes. Login completion uses a controlled local fixture; it does not claim a live account authentication.
