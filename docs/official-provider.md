# Official OpenAI provider

Core discovers a virtual `official` provider from valid ChatGPT OAuth
credentials. No manual provider entry or managed Codex installation is needed.
`providers list --json` is read-only and does not create Mixin or Codex
configuration. API-key-only and managed placeholder auth files are not treated
as ChatGPT OAuth sessions. Account metadata is discovered locally; this is not
a network verification of entitlement or token freshness.

Credential precedence is an explicitly signed-in Mixin session, then the
existing Codex `auth.json`, then the account-specific macOS Codex Keychain
record when file storage is absent. Other platforms support file discovery and
independent browser login; native credential-store import there is not yet
implemented. Credential-store errors are explicit, and no store is enumerated.

Run `codex-mixin --no-tui providers login official --json` or use a UI's OAuth
menu. The CLI opens the authorization page and waits on the official loopback
callback at `localhost:1455/auth/callback`. It validates state and uses PKCE.
Failure to bind the port is explicit. The attempt times out after five minutes;
the terminal also shows the authorization URL for manual browser opening.
Login requires a browser able to reach the machine's loopback callback.

Mixin saves independent login credentials privately in its own OAuth directory,
not in Codex's login file. Owned sessions renew tokens under a bounded
cross-process credential lock. Imported official sessions remain read-only:
Mixin uses their current access token but does not rotate their refresh token.
If an imported token expires before the official app renews it, the request
fails with instructions to refresh the official session or sign in independently
through Mixin. This avoids invalidating another application's refresh token.

When authenticated, model refresh uses the account's native Codex models API.
Account-bound caches are rejected after an account change. A local Codex model
cache can supply initial metadata, and the public repository remains available
for catalog preparation without a login. Real inference always validates auth.
Quota uses the native usage API and renders remaining percentages and reset
times through the common CLI quota contract; neither path launches Codex CLI.

The UI OAuth options are declared by `interface --json`; macOS, Windows and the
Web panel consume that catalog and invoke the same `providers login` command.
Login failure after credentials have been saved reports the failing stage and
`committed: true`. Browser authorization still requires user interaction.

References: [OpenAI authentication documentation](https://learn.chatgpt.com/docs/auth),
[official callback/PKCE implementation](https://github.com/openai/codex/blob/main/codex-rs/login/src/server.rs),
[official credential storage](https://github.com/openai/codex/blob/main/codex-rs/login/src/auth/storage.rs),
[CC Switch native model API](https://github.com/farion1231/cc-switch/blob/main/src-tauri/src/services/codex_oauth_models.rs).

Validation uses temporary homes and configuration paths. `tests/oauth_discovery.rs`
checks discovery, account-bound models and absence of configuration writes.
Callback tests verify that another session's state cannot consume the callback.
Real browser authorization and real provider access remain separate manual checks.
