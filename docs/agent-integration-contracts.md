# Agent integration contracts

The catalog contains 50 Agents in both default and `client-auth` builds: 47 gateway/native-app integrations and three vendor-native model managers. Configuration fixtures and local API fixtures exercise the production adapters; this is not a claim that all 50 vendor applications have been exercised live. App prerequisites and pending states remain part of the contract.

The newly completed contracts were checked against [Magpie at 9f25de5](https://github.com/yetone/magpie/tree/9f25de56c719c67e4e331a360c0c0ee74f4f79e8/internal/agent), pinned on 2026-10-29. Magpie is an integration reference; vendor documentation linked below remains the authority where available.

## Newly connected Agents

Run commands from an installed `codex-mixin` environment with an installed Agent, a configured provider, and selected routable models:

```sh
codex-mixin agent connect reasonix
codex-mixin agent disconnect reasonix
```

The same connect/disconnect commands accept the IDs below. Restart the Agent after configuration changes. Mixin does not install a missing extension or synthesize vendor credentials.

## Native application discovery

`agent list --json` reports installed programs independently of integration state. `cli.path` / `cli.version` describe the command-line installation; `cli.app_path` / `cli.app_version` describe the desktop installation. An application can be detected without a CLI or readable version. Saved configuration and integration journals alone do not establish that software is installed.

- **macOS:** NSWorkspace resolves LaunchServices-registered bundle identifiers and application names, including user/custom installation locations; NSBundle reads the application version. The platform bridge calls these system APIs through JXA, without launching the application.
- **Windows:** the native Registry provider reads per-user/system Uninstall and App Paths registrations, including 32-bit registrations. Executable version resources supply the version, with registered DisplayVersion when resources omit it. The PackageManager-backed `Get-AppxPackage` cmdlet covers matching per-user MSIX registrations. Stale registrations without an existing executable/package are ignored.
- **Linux:** the system Python GIO binding (`python3-gi` on Debian/Ubuntu) uses `Gio.AppInfo` / `Gio.DesktopAppInfo` to resolve registered desktop entries and executable availability, respecting the desktop's XDG/Flatpak/Snap exports. There is no custom `.desktop` or `Exec` parser. Version is unknown unless the entry supplies `X-AppImage-Version` / `X-Version`. Missing GIO is reported as discovery unavailable, not a successful absent-app result.

Desktop registration currently covers Codex/ChatGPT, Claude, DSH, OpenCode, OpenChamber, Goose, Cursor editor, Zed, VS Code/Insiders, VSCodium, Air, Reasonix, Qoder/CN, ZCode, WorkBuddy, CodeBuddy, Pencil, T3 Code, Hanako, Alma and Cindy. Vendor names/identifiers remain an explicit catalog, not substring matching against every installed product. A plugin without a separately registered desktop application is not inferred from its host editor. Unregistered portable programs may need OS registration before they can be discovered as desktop applications.

CLI lookup preserves absolute PATH entry precedence, then checks standard native user/system CLI directories for desktop sessions with a reduced PATH. Relative/current-directory PATH entries and non-executable Unix files are not eligible for background version probes. Desktop paths are never used as CLI launchers or automatic CLI upgrade targets.

Older pending Cindy connections used an import ID longer than Cindy accepts. Connect again to regenerate the private handoff with a valid ID, then confirm the import in Cindy. Installation detection alone does not mark that connection as managed.

| ID | Actual configuration contract | Boundary and evidence |
| --- | --- | --- |
| `cline` | `${CLINE_DATA_DIR:-$CLINE_DIR/data}` with `CLINE_DIR` defaulting to `~/.cline`; `settings/providers.json` version 1, `settings/models.json`, global state and `secrets.json` | CLI/SDK configuration, not VS Code encrypted extension storage. OpenAI-compatible provider slot, model registry and legacy state are coordinated. Unknown provider-file versions fail closed. [Official provider settings](https://github.com/cline/cline/blob/main/sdk/packages/core/src/services/storage/provider-settings-manager.ts), [model registry](https://github.com/cline/cline/blob/main/sdk/packages/core/src/services/providers/local-provider-registry.ts). |
| `vscodium` | OS-specific VSCodium User settings, `chat.languageModels` provider groups | Requires an already installed `GitHub.copilot-chat` extension declaring vendor `customendpoint` or `customoai`; missing/unknown declarations fail without writes. Uses supported per-model request headers rather than claiming to populate VS Code SecretStorage. [Official extension provider declarations](https://github.com/microsoft/vscode-copilot-chat/blob/main/package.json), [custom OpenAI fields](https://github.com/microsoft/vscode-copilot-chat/blob/main/src/extension/byok/vscode-node/customOAIProvider.ts). Extension availability/licensing remains upstream-dependent. |
| `reasonix` | `REASONIX_HOME`, otherwise `~/.reasonix` on macOS/Linux or `%APPDATA%/reasonix` on Windows; `config.toml` plus `.env` | Appends one `[[providers]]` entry, selects `default_model` and `agent.planner_model`, and extends an existing desktop provider allowlist. Credential lives only in `.env`; existing providers and non-managed environment lines survive. Project/session overrides are not changed. [Official paths and credential contract](https://github.com/esengine/DeepSeek-Reasonix/blob/studio/docs/CONFIG_PATHS.md), [provider fields](https://github.com/esengine/DeepSeek-Reasonix/blob/studio/internal/contract/config/provider_entry.go). |
| `minimax-code` | `MINIMAX_DATA_DIR`, then `MAVIS_DATA_DIR`, otherwise `~/.minimax`; `config.yaml` | `custom_provider.codex-mixin`, `api: openai-completions`, `options.baseURL/apiKey`, model table, `defaultModel: custom_provider:codex-mixin/<flat-id>`. Uses reversible gateway aliases for slash-containing catalog IDs; does not modify the slug algorithm. Explicit data-dir overrides can select a non-default profile; profile auto-discovery is not implemented. [Official BYOK guide and data directory](https://github.com/MiniMax-AI/MiniMax-Code#bring-your-own-key), [official custom-provider examples](https://github.com/MiniMax-AI/MiniMax-Code/blob/main/docs/examples.md). Selection/flattening contract was additionally cross-checked against [Magpie's adapter](https://github.com/yetone/magpie/blob/main/internal/agent/minimax.go), a secondary source. |
| `commandcode` | `~/.commandcode/providers.json` and `settings.json`; local-only managed launch | `apiKey: false` by default, or `$CODEX_MIXIN_COMMANDCODE_API_KEY` with client auth. The launcher sets `CMD_LOCAL_ONLY=1`. A conflicting stored native credential is rejected before writes. [Official BYOK documentation](https://commandcode.ai/docs/byok). |
| `fx` | `~/.fx/settings.json`, configured OpenAI Chat provider and model metadata | At most 256 selected models. Native auth is `{type:none}` or `{type:bearer,env:CODEX_MIXIN_FX_API_KEY}`. Authenticated sessions require managed launch. [Official schema](https://github.com/vercel-labs/fx/blob/main/src/core/config/configured_provider.zig). |
| `empryo` | `~/.empryo/config.json` provider array and `defaultModel` | Authenticated configuration references `CODEX_MIXIN_EMPRYO_API_KEY` through `envVar`; managed launch supplies it. Explicit `/v1/models` discovery avoids upstream base stripping. [Official custom provider implementation](https://github.com/proxysoul/Empryo/blob/main/src/core/llm/providers/custom.ts). |
| `ante` | `${ANTE_HOME:-~/.ante}/catalog.json` and `settings.json` | `wire_style: OpenAiCompatible`; authenticated configuration uses `auth.bearer.env_key: CODEX_MIXIN_ANTE_API_KEY` and managed launch. [Official catalog reference](https://docs.antigma.ai/reference/catalog-reference). |

## Completed gateway and native-app adapters

| ID | Native contract | Preconditions and recovery |
| --- | --- | --- |
| `omo` | Native `models.json` and `settings.json` in the first nonblank, trimmed `OMO_CODING_AGENT_DIR` / `SENPI_CODING_AGENT_DIR`, otherwise `~/.omo/agent`; relative overrides resolve from the current working directory | New bindings own `providers.codex-mixin-omo` and select that provider, preserving Pi's imported `providers.codex-mixin`, authentication and unrelated settings. Disconnect restores the previous selection and original files. Native OmO also falls back to `PI_CODING_AGENT_DIR`; Mixin rejects that fallback or an explicitly shared Pi directory before writing. Set a separate `OMO_CODING_AGENT_DIR` for both processes. Existing path checks reject symlinks and parent components. Older bindings keep their captured journal until disconnected; reconnect to use the independent provider. [Native directory precedence](https://github.com/code-yeongyu/oh-my-openagent/blob/19952a1cd940ff435429a1db10b8439e6a4da53b/packages/omo-native/bin/lib/agent-dir.js). |
| `gemini` | `~/.gemini/settings.json`, `~/.gemini/.env`, Gemini Developer API ingress | Writes `security.auth.selectedType=gemini-api-key`, selected model, base URL and gateway key. Native `topK` and `thinkingConfig` survive Gemini forwarding; other protocols receive canonical sampling and reasoning effort. Unsupported official Responses `topK` is rejected. `countTokens` is an estimate, identified by `x-codex-mixin-token-count: estimated`, not the vendor's billing tokenizer. |
| `agy` | Gemini-provider mode in `~/.gemini/antigravity-cli/settings.json` | Custom aliases use `codex-mixin/<catalog-id>`. Managed launch supplies `GOOGLE_GEMINI_BASE_URL`, `GEMINI_API_KEY` and mandatory `--model`; a reversible terminal entry injects the private environment without replacing the native binary. |
| `cursor-local` | `CURSOR_LOCAL_AGENT_BASE_URL` and `CURSOR_LOCAL_AGENT_API_KEY` | Fully quit Cursor, open a new terminal, then run `cursor`. Dock/Start-menu/desktop-icon launches are outside this terminal-only contract. Existing editor processes retain their original environment. |
| `zed` | Native settings plus OS credential store keyed by API URL | The private recovery record captures the prior credential before mutation. Connect/status confirm settings and credential readback; disconnect restores both and refuses credential drift. |
| `aside` | Captured `~/.aside/u/<account>` models/settings plus bounded native REPL | Offline writes report `pending`, not a confirmed live connection. Runtime confirmation is required for `managed`. Disconnect restores the captured account even after active-account changes. |
| `muse` | Native `endpoint_transport`, schema version 1 and `/muse-code/models` | Discovery exposes required Muse metadata and model limits. Managed launch supplies only the gateway `META_API_KEY`, so a vendor sign-in token is not reused for the gateway. Unknown settings schema versions fail closed. |
| `alma` | Running loopback API on port 23001 | Unique owned provider, model list and chat selection are confirmed. A durable preparing record permits retry after partial API failure without duplicate providers. Disconnect preserves unrelated settings and restores the prior default. |
| `hanako` | Running authenticated loopback API identified through `server-info.json` | The encrypted provider catalog is never rewritten. The captured agent/model is restored through native APIs. If no replacement chat model exists, disconnect reports `pending_removal` after access revocation until the user chooses a native replacement. |
| `cindy` | `cindy://provider/import` through a private HTML handoff | Confirm import in Cindy. Status matches the owned slug and native eight-hex ID suffix, then verifies all three runtime routes in read-only SQLite. Ambiguous imports, edited routes and unreadable databases remain errors. Disconnect revokes gateway access and reports `pending_removal` until the user removes the native provider. The encrypted database is never written. |

## Vendor-native model management

Cursor CLI (`cursor`), Copilot CLI (`copilot`) and Devin (`devin`) do not expose a verified general custom-backend endpoint. Their supported capability is explicitly `native_models`, using the vendor's account and model IDs. No gateway readiness, selected gateway model or client credential is required. Native files are `~/.cursor/cli-config.json`, `~/.copilot/settings.json`, and `$XDG_CONFIG_HOME/devin/config.json` (default `~/.config/devin/config.json`); Devin uses `%APPDATA%/devin/config.json` on Windows. Connecting captures the existing vendor-native model in a real undo journal. If no native model is configured, explicitly select one first; empty marker journals never count as managed.

```sh
codex-mixin agent model cursor --json
codex-mixin agent model cursor --model VENDOR_MODEL_ID
codex-mixin agent model copilot --model VENDOR_MODEL_ID --effort high
codex-mixin agent model devin --model VENDOR_MODEL_VARIANT
codex-mixin agent disconnect cursor
```

Writes touch only native model fields and use the existing private undo journal. Repeated selections preserve unrelated user edits; disconnect restores the original model and leaves vendor credentials intact.

## Managed process environments

```sh
codex-mixin agent connect agy
codex-mixin agent run agy
codex-mixin agent launch cursor-local
```

Terminal startup is required for agy, Cursor, Muse, Command Code, and authenticated fx/Empryo/Ante. Connecting installs reversible user zsh/bash/fish/PowerShell entries that invoke the same CLI; open a new terminal, then use the ordinary command. Entries preserve arguments and the selected Mixin configuration and contain no keys. Cursor uses `cursor`, never `cursor-agent`; fully quit existing editor processes first. Desktop entry points and Windows cmd are not intercepted. `agent run` inherits terminal I/O and `agent launch` opens a terminal explicitly. Disconnect removes only unchanged owned profile blocks, preserves unrelated edits, restores the managed configuration, and revokes that client's key. Open a new terminal after disconnecting.

## State contract

- `managed`: the adapter confirmed its owned fields, native credential or live native selection.
- `pending`: configuration/import is prepared but the native runtime or user has not confirmed it.
- `pending_removal`: gateway access has been revoked; native cleanup still needs user action.
- `drift` / `error`: managed data changed or could not be inspected. These states do not count as successful confirmation.
- `mode: native_models`: vendor-native selection management, not custom gateway routing.

All shells consume these states from the CLI. No shell implements native configuration or credential policy.

## Preservation, security and verification

The existing durable integration journal preflights all files, records original content, writes private files, and restores on disconnect. Original untouched files return byte-for-byte. Non-managed field changes are retained; managed-field drift is reported instead of overwritten. Array-owned provider lists are restored only if the managed list still matches; concurrent array edits fail closed rather than deleting user entries. Credential and journal permissions follow the existing private-file writer.

Reasonix `.env` parsing preserves comments and unrelated assignment contents, normalizing CRLF to LF when files are rewritten. Duplicate variable names, malformed lines and multiline assignments, including escaped closing quotes, fail before any write; this intentionally narrower syntax avoids ambiguous restore semantics. Existing managed-key collisions are rejected. The adapter never places the Reasonix API key in its TOML configuration.

Full validation runs in GitHub CI. Existing default and `client-auth` full-target gates, strict Clippy, architecture checks and protocol/cache E2E remain required. Local validation of native discovery is limited to an APP build and read-only discovery queries; it is not evidence of Windows/Linux runtime coverage or a completed Cindy import. Adjacent non-UI tests cover native model preservation, private launch validation, Gemini options/token estimates, Muse discovery, Zed credential rollback, Aside account staging, Cindy import ID migration/read-only route confirmation, Alma partial failures and Hanako restoration/removal. Tests use temporary paths, in-memory credential callbacks and loopback servers; they do not invoke real credential stores or vendor applications.

Native compatibility: Gemini model picker registration uses visible custom `modelDefinitions`, `customAliases`, and `experimental.dynamicModelConfiguration`. Disconnect/reconnect older Gemini bindings to install these fields. Zed owns default-model fields individually, including native thinking/effort settings; unrelated native migration fields are preserved. Existing deprecated sidebar settings belong to Zed migration, and a real user model change remains a managed-field conflict.
