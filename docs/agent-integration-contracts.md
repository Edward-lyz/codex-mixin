# Remaining 20 Agent integration contracts

Status: configuration-adapter implementation and targeted fixture validation. This is not a claim that all 20 Agents support custom backends, nor that live vendor applications have been tested. The catalog contains 50 Agents. The default build has 38 configurable integrations; `client-auth` builds have 34 because four Agents need a managed authenticated launcher.

## Newly connected Agents

Run commands from an installed `codex-mixin` environment with an installed Agent, a configured provider, and selected routable models:

```sh
codex-mixin agent connect reasonix
codex-mixin agent disconnect reasonix
```

The same connect/disconnect commands accept the IDs below. Restart the Agent after configuration changes. Installation/version detection still follows each Agent's existing installation channel; Mixin does not install a missing extension or synthesize vendor credentials.

| ID | Actual configuration contract | Boundary and evidence |
| --- | --- | --- |
| `cline` | `${CLINE_DATA_DIR:-$CLINE_DIR/data}` with `CLINE_DIR` defaulting to `~/.cline`; `settings/providers.json` version 1, `settings/models.json`, global state and `secrets.json` | CLI/SDK configuration, not VS Code encrypted extension storage. OpenAI-compatible provider slot, model registry and legacy state are coordinated. Unknown provider-file versions fail closed. [Official provider settings](https://github.com/cline/cline/blob/main/sdk/packages/core/src/services/storage/provider-settings-manager.ts), [model registry](https://github.com/cline/cline/blob/main/sdk/packages/core/src/services/providers/local-provider-registry.ts). |
| `vscodium` | OS-specific VSCodium User settings, `chat.languageModels` provider groups | Requires an already installed `GitHub.copilot-chat` extension declaring vendor `customendpoint` or `customoai`; missing/unknown declarations fail without writes. Uses supported per-model request headers rather than claiming to populate VS Code SecretStorage. [Official extension provider declarations](https://github.com/microsoft/vscode-copilot-chat/blob/main/package.json), [custom OpenAI fields](https://github.com/microsoft/vscode-copilot-chat/blob/main/src/extension/byok/vscode-node/customOAIProvider.ts). Extension availability/licensing remains upstream-dependent. |
| `reasonix` | `REASONIX_HOME`, otherwise `~/.reasonix` on macOS/Linux or `%APPDATA%/reasonix` on Windows; `config.toml` plus `.env` | Appends one `[[providers]]` entry, selects `default_model` and `agent.planner_model`, and extends an existing desktop provider allowlist. Credential lives only in `.env`; existing providers and non-managed environment lines survive. Project/session overrides are not changed. [Official paths and credential contract](https://github.com/esengine/DeepSeek-Reasonix/blob/studio/docs/CONFIG_PATHS.md), [provider fields](https://github.com/esengine/DeepSeek-Reasonix/blob/studio/internal/contract/config/provider_entry.go). |
| `minimax-code` | `MINIMAX_DATA_DIR`, then `MAVIS_DATA_DIR`, otherwise `~/.minimax`; `config.yaml` | `custom_provider.codex-mixin`, `api: openai-completions`, `options.baseURL/apiKey`, model table, `defaultModel: custom_provider:codex-mixin/<flat-id>`. Uses reversible gateway aliases for slash-containing catalog IDs; does not modify the slug algorithm. Explicit data-dir overrides can select a non-default profile; profile auto-discovery is not implemented. [Official BYOK guide and data directory](https://github.com/MiniMax-AI/MiniMax-Code#bring-your-own-key), [official custom-provider examples](https://github.com/MiniMax-AI/MiniMax-Code/blob/main/docs/examples.md). Selection/flattening contract was additionally cross-checked against [Magpie's adapter](https://github.com/yetone/magpie/blob/main/internal/agent/minimax.go), a secondary source. |
| `commandcode` | `~/.commandcode/providers.json` provider entry and `settings.json` model selection | Default build only: `apiKey: false`, `api: openai-completions`. [Official BYOK documentation](https://commandcode.ai/docs/byok). |
| `fx` | `~/.fx/settings.json`, configured OpenAI Chat provider, `auth.type: none`, model metadata | Default build only. At most 256 selected models. [Official configured-provider schema](https://github.com/vercel-labs/fx/blob/main/src/core/config/configured_provider.zig). |
| `empryo` | `~/.empryo/config.json` provider array and `defaultModel` | Default build only. No `envVar` credential is invented; explicit `/v1/models` discovery avoids upstream's `/v1` stripping. Availability probes the provider base URL and accepts 2xx/401/403; gateway GET `/v1` must be present. [Official custom provider implementation](https://github.com/proxysoul/Empryo/blob/main/src/core/llm/providers/custom.ts). |
| `ante` | `${ANTE_HOME:-~/.ante}/catalog.json` provider/model catalog and `settings.json` | Default build only; `wire_style: OpenAiCompatible`. [Official catalog reference](https://docs.antigma.ai/reference/catalog-reference). |

CommandCode, FX, Empryo and Ante are deliberately rejected when `client-auth` is compiled in. Writing a configuration value that these applications interpret as an environment-variable name would not actually install a credential. Existing managed journals remain inspectable and removable across build-feature changes. Mixin never edits a shell profile or starts a process with an unapproved injected key.

## Remaining explicit limitations (12)

| ID | Why connection remains unavailable |
| --- | --- |
| `gemini` | Gemini Developer API ingress exists, but the CLI's `topK`/`thinkingConfig` and auxiliary `countTokens` paths are not supported end to end. Do not equate the wire ingress with a working Gemini CLI integration. |
| `agy` | Antigravity requires its CloudCode transport/authentication, not just the Gemini Developer API. |
| `cursor`, `copilot`, `devin` | Their supported CLI selection paths expose vendor-native models, not a verified general custom-backend contract. |
| `cursor-local` | Requires process environment injection and a managed Cursor launcher. |
| `zed` | Needs a credential-store adapter; settings alone do not install its API key. |
| `aside` | Needs the native runtime connection/staging API. |
| `muse` | Needs `/muse-code/models` discovery and an authenticated transport adapter. |
| `hanako` | Needs the running app API for its keychain-encrypted catalog. |
| `alma` | Needs the running local provider-import/removal API. |
| `cindy` | Needs `cindy://provider/import` user confirmation; the provider database is encrypted. |

For the app/API-only restrictions, current evidence includes [Magpie's upstream adapters](https://github.com/yetone/magpie/tree/main/internal/agent), a **secondary integration source**, not official vendor validation. These entries are intentionally not marked supported. Their installation/version management remains independent from gateway integration.

## Preservation, security and verification

The existing durable integration journal preflights all files, records original content, writes private files, and restores on disconnect. Original untouched files return byte-for-byte. Non-managed field changes are retained; managed-field drift is reported instead of overwritten. Array-owned provider lists are restored only if the managed list still matches; concurrent array edits fail closed rather than deleting user entries. Credential and journal permissions follow the existing private-file writer.

Reasonix `.env` parsing preserves comments and unrelated assignment contents, normalizing CRLF to LF when files are rewritten. Duplicate variable names, malformed lines and multiline assignments, including escaped closing quotes, fail before any write; this intentionally narrower syntax avoids ambiguous restore semantics. Existing managed-key collisions are rejected. The adapter never places the Reasonix API key in its TOML configuration.

Targeted validation uses temporary directories and real parser/journal/public status/remove paths:

```sh
cargo test --lib clients::frameworks -- --test-threads=1
cargo test --lib --features client-auth clients::frameworks -- --test-threads=1
```

These tests cover field mapping, catalog IDs with slashes/dots, Cline legacy-state coordination, extension declaration validation, keyless feature gates, backup/restore, unmanaged content, drift and dotenv fail-fast cases. They do not launch the eight vendor applications or make paid provider requests.

Full-target regression passed with 961 tests in the default build and 962 in `client-auth`, with seven explicitly ignored tests in each build. The commands unset inherited `DSH_HOME` so subprocess fixtures cannot discover the real Harness configuration, and exclude the existing network-dependent `oauth_install_missing_repository_and_cache_creates_no_restore_marker_or_directory` test: its supposed failing fetch returned a real remote catalog in this environment. Neither ignored nor excluded tests count as passed. Both builds passed strict all-target Clippy; formatting and architecture positive/negative checks also passed.

The separate Node prompt-cache E2E script could not start its mock providers: the installed Node runtime aborts because `libsimdutf.36.dylib` is missing. The Rust HTTP/SSE tests, including prompt-prefix regressions, passed; they do not substitute for a successful run of that script.
