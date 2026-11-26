<p align="center">
  <img src="docs/assets/app-icon.png" width="96" alt="Codex Mixin Logo">
</p>

<h1 align="center">Codex Mixin</h1>

<p align="center">
  <b>Use any model in official Codex. Official GPT models and ChatGPT sign-in stay as they are.</b><br>
  A local model gateway written in Rust: configure providers once, and Codex plus 50 coding agents share one model list.
</p>

<p align="center">
  <a href="README.md">简体中文</a> | <b>English</b>
</p>

<p align="center">
  <a href="https://github.com/Edward-lyz/codex-mixin/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/Edward-lyz/codex-mixin?color=blue&label=version"></a>
  <a href="https://github.com/Edward-lyz/codex-mixin/releases"><img alt="Platforms" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey"></a>
  <a href="https://github.com/Edward-lyz/codex-mixin/wiki"><img alt="Docs" src="https://img.shields.io/badge/docs-GitHub%20Wiki-blueviolet"></a>
</p>

<p align="center">
  <a href="docs/assets/APP-Console-Routing.png"><img src="docs/assets/APP-Console-Routing.png" width="1000" alt="Codex Mixin failover routing with a primary model and ordered fallbacks"></a>
</p>

<p align="center">
  <a href="#core-capabilities">Core Capabilities</a> ·
  <a href="#resource-usage">Resource Usage</a> ·
  <a href="#quick-start">Quick Start</a> ·
  <a href="#interface">Interface</a> ·
  <a href="#providers">Providers</a> ·
  <a href="#agent-integrations">Agents</a> ·
  <a href="#documentation">Docs</a> ·
  <a href="#faq">FAQ</a>
</p>

---

## Core Capabilities

Codex Mixin runs on your machine, between your agents and your model providers. It converts requests and streaming responses across OpenAI Responses, Chat Completions, Anthropic Messages, and Gemini, including reasoning content and tool calls. The macOS, Windows, and Linux desktop apps share the gateway core with the CLI and web panel.

### Official Codex stays intact

Connect Codex in Official Account mode and your custom models join the official model picker, next to GPT. ChatGPT sign-in, session history, mobile remote tasks, and voice keep working. Connecting and disconnecting never touch the Codex session database or account state; after you disconnect, the Codex configuration returns to the official OpenAI defaults.

### Model routing: one model name, a group of models behind it

Each route appears as an `auto/<name>` model that every connected client can pick from its model list. All three interfaces let you edit routes directly on a flow chart; the CLI uses `codex-mixin route`.

| Route type | Behavior |
| --- | --- |
| Failover | When the source model errors before producing output (including exhausted quota), or produces nothing for 30 seconds, the gateway switches to 1–4 fallback models in order |
| Fusion | 1–8 models answer in parallel, a judge model reviews, and a final model revises and replies |
| Schedule | Requests go to different models by local time, across 1–24 time slots |

See [Model routing](docs/model-routing.md) for triggers, limits, and coverage.

### Context engine: compress history, keep the originals restorable

The gateway keeps an archive of original messages and a compressed view for each session. The model compresses older history itself through internal tools injected by the gateway, and searches or restores the originals when it needs them; the most recent 10,000 tokens always stay verbatim. Compression has three tiers, with rules ported from Billion Context.

The engine is off by default. Enable it with `codex-mixin context config set --enabled true` or the console switch. See [Context engine](docs/context-engine.md) for supported request paths and every setting.

<table>
  <tr>
    <td width="50%" align="center">
      <a href="docs/assets/APP-Console-Context.png"><img src="docs/assets/APP-Console-Context.png" alt="Context engine settings and session statistics in the desktop console"></a><br>
      <sub><b>Console</b> · Sessions, compressions, tokens saved</sub>
    </td>
    <td width="50%" align="center">
      <a href="docs/assets/context-engine-comparison.svg"><img src="docs/assets/context-engine-comparison.svg" alt="Built-in compaction compared with archived, tiered context management"></a><br>
      <sub><b>Mechanism</b> · Built-in compaction vs. archived tiers</sub>
    </td>
  </tr>
</table>

### Usage, reply by reply

The latest 100,000 replies are kept locally with tokens, cache hits, reasoning cost, first-token latency, and output speed for each one. Failed replies are recorded too. The menu bar and tray show provider quota directly.

---

## Resource Usage

On November 24, 2026, Codex Mixin 1.0.0, Magpie 0.1.4734, and CC Switch 4.0.40 were measured against a controlled local upstream on an Apple M2 with 16 GiB RAM and macOS 27.2. Each request contained 1,000,000 text tokens and two 1024 × 768 PNGs, at 0, 10, 20, and 40 concurrent requests.

In **standard mode**, the full desktop UI was loaded and its window hidden:

| Measurement | Codex Mixin | Magpie | CC Switch |
| --- | ---: | ---: | ---: |
| Idle memory | **160.9 MiB** | 182.5 MiB | 182.5 MiB |
| 10 active requests | **164.9 MiB** | 390.3 MiB | 317.3 MiB |
| 20 active requests | **167.7 MiB** | 397.3 MiB | 346.0 MiB |
| 40 active requests | **175.6 MiB** | 637.6 MiB | 602.7 MiB |
| CPU · 40 active requests | **1.492%** | 5.822% | 1.721% |

<p align="center">
  <a href="docs/benchmarks/2026-11-24-desktop-resource-repeat/normal-en.svg"><img src="docs/benchmarks/2026-11-24-desktop-resource-repeat/normal-en.svg" width="1000" alt="Standard mode physical memory and gateway CPU at 0, 10, 20, and 40 active requests"></a>
</p>

In **lightweight mode**, the main window was closed and the background service kept running. Magpie sampling began after its WebContent and GPU processes had both exited:

| Measurement | Codex Mixin | Magpie | CC Switch |
| --- | ---: | ---: | ---: |
| Idle memory | **6.6 MiB** | 100.3 MiB | 30.2 MiB |
| 10 active requests | **10.2 MiB** | 273.7 MiB | 140.2 MiB |
| 20 active requests | **15.3 MiB** | 312.1 MiB | 299.0 MiB |
| 40 active requests | **24.8 MiB** | 549.7 MiB | 382.4 MiB |
| CPU · 40 active requests | 1.884% | 6.371% | **1.778%** |

<p align="center">
  <a href="docs/benchmarks/2026-11-24-desktop-resource-repeat/lightweight-en.svg"><img src="docs/benchmarks/2026-11-24-desktop-resource-repeat/lightweight-en.svg" width="1000" alt="Lightweight mode physical memory and gateway CPU at 0, 10, 20, and 40 active requests"></a>
</p>

Memory is the macOS physical footprint of the application process group; CPU is the single-core percentage for the gateway process and its waited-for children. Each value is the median of three consecutive 20-second window means. Each mode was measured in one ascending load run, so history and caches accumulated; all 210 requests in each run passed validation. The [benchmark report](docs/benchmarks/2026-11-24-desktop-resource-repeat/README.md) contains the method, window ranges, raw samples, and validation data.

---

## Quick Start

### Install

Download the package for your platform from [Releases](https://github.com/Edward-lyz/codex-mixin/releases/latest). You do not need Rust or a source build.

| Platform | Install | Entry point |
| --- | --- | --- |
| macOS | Download the Apple Silicon or Intel `.dmg` and drag the app to Applications | Menu bar icon → Open Console |
| Windows | Choose x86_64 or ARM64 (`aarch64`). Run `setup.exe` to install for your account, or extract the portable ZIP and run `Codex Mixin.exe` | System tray icon → Open Console |
| Linux desktop | Extract the x86_64 or ARM64 `codex-mixin-app-*-linux-gnu.tar.gz` and run `./install.sh` | Application launcher → Codex Mixin |
| Linux CLI | Extract `codex-mixin-cli-*-linux-musl.tar.gz` or install the `.deb`, then run `codex-mixin panel` | Open the address printed in the terminal |

```bash
# Homebrew (macOS / Linux): CLI; use --cask for the desktop app
brew tap edward-lyz/codex-mixin https://github.com/Edward-lyz/codex-mixin
brew install codex-mixin            # or: brew install --cask codex-mixin

# Docker: 8317 is the gateway, 8318 the control panel; get the panel URL and token from docker logs codex-mixin
docker run -d --name codex-mixin -p 127.0.0.1:8317:8317 -p 127.0.0.1:8318:8318 -v codex-mixin:/data ghcr.io/edward-lyz/codex-mixin
```

See [Installation](https://github.com/Edward-lyz/codex-mixin/wiki/Installation) for system requirements, file names, and remote access.

### Set up in the console

1. On the Providers page, add a provider with an API key or OAuth sign-in.
2. Select the models you want in the model list and save.
3. On the Agents page, connect an installed client. For Codex, choose Official Account mode.
4. Restart the client or open a new session, then pick a custom model in the model picker.

To restore a client's original configuration, disconnect it on the same page.

### Or use only the command line

```sh
curl -fsSL https://raw.githubusercontent.com/Edward-lyz/codex-mixin/main/scripts/install.sh | sh
codex-mixin provider add --preset deepseek --key sk-...   # discovers models automatically
codex-mixin connect codex
codex-mixin service start
curl http://127.0.0.1:8317/v1/models                      # a model list means it works
```

<details>
<summary>Ports, remote access, and access keys</summary>

The gateway listens only on `127.0.0.1:8317` by default. The Web panel listens on all interfaces on port 8318 and requires a token generated for each start. If a port is taken, either one falls back to a free port in 8000–9000.

To let other devices reach the gateway, start it with `codex-mixin service start --bind 0.0.0.0:PORT`; the listen address is saved. When the gateway listens beyond localhost, its first start generates an `sk-` access key and prints it to the terminal. Requests from the local machine and private networks (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16, fc00::/7, fe80::/10) need no key; any other source must send the key in the `Authorization: Bearer <key>` or `x-api-key` header, or the gateway returns 401. Manage multiple keys with `codex-mixin config keys list / create / revoke` or the Access Keys page of the control panel.

See [Data and Security](https://github.com/Edward-lyz/codex-mixin/wiki/Data-and-Security) for the access boundaries.

</details>

---

## Interface

The macOS, Windows, and Linux desktop apps share a GPUI interface, so the layout and workflows are consistent across all three platforms. Use the console to manage providers and models, connect agents, review usage, and configure the context engine and routing. The CLI and web panel can also manage the gateway.

<table>
  <tr>
    <td width="50%" align="center">
      <a href="docs/assets/APP-Console-Models.png"><img src="docs/assets/APP-Console-Models.png" alt="Providers page showing selected models and their context windows"></a><br>
      <sub><b>Providers</b> · Model selection, context windows, and capabilities</sub>
    </td>
    <td width="50%" align="center">
      <a href="docs/assets/APP-Console-Agents.png"><img src="docs/assets/APP-Console-Agents.png" alt="Agents page showing installed client versions and connection toggles"></a><br>
      <sub><b>Agents</b> · Installed versions and connection status</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <a href="docs/assets/APP-Console-Usage.png"><img src="docs/assets/APP-Console-Usage.png" alt="Usage page showing daily token trends and per-model statistics"></a><br>
      <sub><b>Usage</b> · Request counts, token trends, and model statistics</sub>
    </td>
    <td width="50%" align="center">
      <a href="docs/assets/APP-Console-Settings.png"><img src="docs/assets/APP-Console-Settings.png" alt="Settings page showing app updates, gateway controls, configuration backups, and diagnostics"></a><br>
      <sub><b>Settings</b> · Gateway, app updates, and configuration backups</sub>
    </td>
  </tr>
</table>

More screens are in the [Product Tour](https://github.com/Edward-lyz/codex-mixin/wiki/Product-Tour).

---

## Providers

Choose one of these presets when adding a provider, or Custom for any other compatible endpoint. After adding one, you can test the connection, refresh the model list, and probe Reasoning, Function Calling, Tool Search, and Web Search support. Probing sends real requests and may consume quota.

For multiple OAuth accounts, give each additional account a distinct ID, for example `codex-mixin --no-tui providers add --preset openai --id work --name Work` or `codex-mixin --no-tui providers add --preset agy --id agy-2`. Add `--reauthorize` to sign in again. Model slugs include the ID, such as `gpt-5.5-work`. For an independent Claude Code account, obtain a token with `claude setup-token`, then use `--preset claude-code --id work-claude --key @env:CLAUDE_SETUP_TOKEN`.

| Preset | Provider | Upstream protocol | Authentication |
| --- | --- | --- | --- |
| `openai` | OpenAI | OpenAI Responses | ChatGPT OAuth |
| `claude-code` | Claude Code | Anthropic Messages | Reuses the local Claude Code login and calls the official `claude` through the built-in PTY/ConPTY bridge |
| `baidu-oneapi` | Baidu OneAPI | Anthropic Messages; GPT models use OpenAI Responses | API key |
| `openrouter` | OpenRouter | OpenAI Chat Completions | API key |
| `deepseek` | DeepSeek | OpenAI Responses | API key |
| `agy` | Google Antigravity | Gemini | Google OAuth |
| `agy-business` | Antigravity Business | Gemini | Google OAuth or GCP ADC credentials, with project and region required |
| `kiro` | Kiro | Anthropic Messages | Reuses the local Kiro / `kiro-cli` login |
| `opencode-go` | OpenCode Go | OpenAI Responses | API key, with a session affinity header |
| `aws-bedrock` | Amazon Bedrock (Mantle) | Anthropic Messages | Bedrock API key or AWS access key |
| `custom` | Any compatible endpoint | OpenAI Responses (default) | Custom headers and key |

The Claude Code subscription reuses your local login and runs the official interactive `claude` through a built-in PTY/ConPTY bridge, with streaming output, MCP tool round trips, and in-process session reuse. This path requires Claude Code to be installed, and stream capture depends on its Bun runtime; an incompatible runtime produces an explicit error.

---

## Agent Integrations

51 agents are supported. 48 of them use custom models through the gateway, and disconnecting restores the managed fields while keeping unrelated configuration. Cursor CLI, Copilot CLI, and Devin manage vendor-native models through vendor accounts. Prerequisites for each client are in the [Agent contracts](docs/agent-integration-contracts.md).

<details>
<summary>Show all 51 agents</summary>

<table>
  <tr>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/codex.svg" width="36" height="36" alt="Codex logo"><br>
      <sub>Codex</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/claude.svg" width="36" height="36" alt="Claude Code logo"><br>
      <sub>Claude Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/claude-desktop.svg" width="36" height="36" alt="Claude Desktop logo"><br>
      <sub>Claude Desktop</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/dsh.svg" width="36" height="36" alt="DSH logo"><br>
      <sub>DSH</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/opencode-light.svg#gh-light-mode-only" width="36" height="36" alt="OpenCode logo"><img src="docs/assets/agents/opencode-dark.svg#gh-dark-mode-only" width="36" height="36" alt="OpenCode logo"><br>
      <sub>OpenCode</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="docs/assets/agents/pi-light.svg#gh-light-mode-only" width="36" height="36" alt="Pi logo"><img src="docs/assets/agents/pi-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Pi logo"><br>
      <sub>Pi</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/openchamber.svg" width="36" height="36" alt="OpenChamber logo"><br>
      <sub>OpenChamber</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/mimocode-light.svg#gh-light-mode-only" width="36" height="36" alt="MiMo Code logo"><img src="docs/assets/agents/mimocode-dark.svg#gh-dark-mode-only" width="36" height="36" alt="MiMo Code logo"><br>
      <sub>MiMo Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/omo-light.svg#gh-light-mode-only" width="36" height="36" alt="OmO logo"><img src="docs/assets/agents/omo-dark.svg#gh-dark-mode-only" width="36" height="36" alt="OmO logo"><br>
      <sub>OmO</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/goose-light.svg#gh-light-mode-only" width="36" height="36" alt="Goose logo"><img src="docs/assets/agents/goose-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Goose logo"><br>
      <sub>Goose</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="docs/assets/agents/vscode-light.svg#gh-light-mode-only" width="36" height="36" alt="VS Code logo"><img src="docs/assets/agents/vscode-dark.svg#gh-dark-mode-only" width="36" height="36" alt="VS Code logo"><br>
      <sub>VS Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/vscode-insiders-light.svg#gh-light-mode-only" width="36" height="36" alt="VS Code Insiders logo"><img src="docs/assets/agents/vscode-insiders-dark.svg#gh-dark-mode-only" width="36" height="36" alt="VS Code Insiders logo"><br>
      <sub>VS Code Insiders</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/copilot-jetbrains-light.svg#gh-light-mode-only" width="36" height="36" alt="Copilot for JetBrains logo"><img src="docs/assets/agents/copilot-jetbrains-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Copilot for JetBrains logo"><br>
      <sub>Copilot (JetBrains)</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/air.svg" width="36" height="36" alt="Air logo"><br>
      <sub>Air</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/crush.svg" width="36" height="36" alt="Crush logo"><br>
      <sub>Crush</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="docs/assets/agents/omp-light.svg#gh-light-mode-only" width="36" height="36" alt="Oh My Pi logo"><img src="docs/assets/agents/omp-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Oh My Pi logo"><br>
      <sub>Oh My Pi</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/hermes-light.svg#gh-light-mode-only" width="36" height="36" alt="Hermes Agent logo"><img src="docs/assets/agents/hermes-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Hermes Agent logo"><br>
      <sub>Hermes Agent</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/morph.svg" width="36" height="36" alt="Mister Morph logo"><br>
      <sub>Mister Morph</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/kimi-light.svg#gh-light-mode-only" width="36" height="36" alt="Kimi Code logo"><img src="docs/assets/agents/kimi-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Kimi Code logo"><br>
      <sub>Kimi Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/qwen.svg" width="36" height="36" alt="Qwen Code logo"><br>
      <sub>Qwen Code</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="docs/assets/agents/droid-light.svg#gh-light-mode-only" width="36" height="36" alt="Droid logo"><img src="docs/assets/agents/droid-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Droid logo"><br>
      <sub>Droid</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/qoder-light.svg#gh-light-mode-only" width="36" height="36" alt="Qoder logo"><img src="docs/assets/agents/qoder-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Qoder logo"><br>
      <sub>Qoder</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/qoder-cn-light.svg#gh-light-mode-only" width="36" height="36" alt="Qoder CN logo"><img src="docs/assets/agents/qoder-cn-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Qoder CN logo"><br>
      <sub>Qoder CN</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/grok-light.svg#gh-light-mode-only" width="36" height="36" alt="Grok Build logo"><img src="docs/assets/agents/grok-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Grok Build logo"><br>
      <sub>Grok Build</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/zcode.svg" width="36" height="36" alt="ZCode logo"><br>
      <sub>ZCode</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/workbuddy.svg" width="36" height="36" alt="WorkBuddy logo"><br>
      <sub>WorkBuddy</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/codebuddy.svg" width="36" height="36" alt="CodeBuddy logo"><br>
      <sub>CodeBuddy</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/pencil.svg" width="36" height="36" alt="Pencil logo"><br>
      <sub>Pencil</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/t3code.svg" width="36" height="36" alt="T3 Code logo"><br>
      <sub>T3 Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/atomcode.svg" width="36" height="36" alt="AtomCode logo"><br>
      <sub>AtomCode</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="docs/assets/agents/vscodium-light.svg#gh-light-mode-only" width="36" height="36" alt="VSCodium logo"><img src="docs/assets/agents/vscodium-dark.svg#gh-dark-mode-only" width="36" height="36" alt="VSCodium logo"><br>
      <sub>VSCodium</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/reasonix.svg" width="36" height="36" alt="Reasonix logo"><br>
      <sub>Reasonix</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/commandcode-light.svg#gh-light-mode-only" width="36" height="36" alt="Command Code logo"><img src="docs/assets/agents/commandcode-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Command Code logo"><br>
      <sub>Command Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/fx-light.svg#gh-light-mode-only" width="36" height="36" alt="fx logo"><img src="docs/assets/agents/fx-dark.svg#gh-dark-mode-only" width="36" height="36" alt="fx logo"><br>
      <sub>fx</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/empryo.svg" width="36" height="36" alt="Empryo logo"><br>
      <sub>Empryo</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/ante.svg" width="36" height="36" alt="Ante logo"><br>
      <sub>Ante</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/minimax-code.svg" width="36" height="36" alt="MiniMax Code logo"><br>
      <sub>MiniMax Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/cline-light.svg#gh-light-mode-only" width="36" height="36" alt="Cline logo"><img src="docs/assets/agents/cline-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Cline logo"><br>
      <sub>Cline</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/gemini.svg" width="36" height="36" alt="Gemini CLI logo"><br>
      <sub>Gemini CLI</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/agy.svg" width="36" height="36" alt="agy logo"><br>
      <sub>agy</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="docs/assets/agents/cursor-light.svg#gh-light-mode-only" width="36" height="36" alt="Cursor CLI logo"><img src="docs/assets/agents/cursor-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Cursor CLI logo"><br>
      <sub>Cursor CLI · native models</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/cursor-local-light.svg#gh-light-mode-only" width="36" height="36" alt="Cursor logo"><img src="docs/assets/agents/cursor-local-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Cursor logo"><br>
      <sub>Cursor</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/copilot-light.svg#gh-light-mode-only" width="36" height="36" alt="Copilot CLI logo"><img src="docs/assets/agents/copilot-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Copilot CLI logo"><br>
      <sub>Copilot CLI · native models</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/devin-light.svg#gh-light-mode-only" width="36" height="36" alt="Devin logo"><img src="docs/assets/agents/devin-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Devin logo"><br>
      <sub>Devin · native models</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/zed-light.svg#gh-light-mode-only" width="36" height="36" alt="Zed logo"><img src="docs/assets/agents/zed-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Zed logo"><br>
      <sub>Zed</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/aside.svg" width="36" height="36" alt="Aside logo"><br>
      <sub>Aside</sub>
    </td>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/muse.svg" width="36" height="36" alt="Muse logo"><br>
      <sub>Muse</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/hanako.svg" width="36" height="36" alt="Hanako logo"><br>
      <sub>Hanako</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/alma.svg" width="36" height="36" alt="Alma logo"><br>
      <sub>Alma</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/cindy.svg" width="36" height="36" alt="Cindy logo"><br>
      <sub>Cindy</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="desktop/assets/agents/agy.svg" width="36" height="36" alt="Antigravity logo"><br>
      <sub>Antigravity</sub>
    </td>
  </tr>
</table>

</details>

---

## Documentation

Guides live in the [GitHub Wiki](https://github.com/Edward-lyz/codex-mixin/wiki):

- [Installation](https://github.com/Edward-lyz/codex-mixin/wiki/Installation) and [Quick Start](https://github.com/Edward-lyz/codex-mixin/wiki/Quick-Start)
- [Providers and models](https://github.com/Edward-lyz/codex-mixin/wiki/Providers-and-Models)
- [Client integrations](https://github.com/Edward-lyz/codex-mixin/wiki/Client-Integrations)
- [Usage and quota](https://github.com/Edward-lyz/codex-mixin/wiki/Usage-and-Quota)
- [Configuration backup and restore](https://github.com/Edward-lyz/codex-mixin/wiki/Configuration-Backup-and-Restore)
- [CLI reference](https://github.com/Edward-lyz/codex-mixin/wiki/CLI-Reference)
- [Troubleshooting](https://github.com/Edward-lyz/codex-mixin/wiki/Troubleshooting) and [FAQ](https://github.com/Edward-lyz/codex-mixin/wiki/FAQ)

Design and mechanism documents live in the repository:

- [Architecture and module boundaries](docs/architecture.md)
- [Model routing](docs/model-routing.md)
- [Long-context engine](docs/context-engine.md)
- [Agent integration contracts](docs/agent-integration-contracts.md)
- [Google Antigravity](docs/antigravity.md)
- [ECH for official GPT (experimental)](docs/ech-gpt.md)
- [Browser CORS allowlist](docs/cors.md)

For version changes, see [RELEASE_NOTES.md](RELEASE_NOTES.md).

---

## FAQ

<details>
<summary>Can I keep ChatGPT sign-in and official GPT models?</summary>

Yes. Connect Codex in Official Account mode. Custom models join the official model picker, while sign-in, session history, and mobile remote tasks remain available. Connecting and disconnecting do not change the Codex session database or account state. See [Client integrations](https://github.com/Edward-lyz/codex-mixin/wiki/Client-Integrations).

</details>

<details>
<summary>Do all 51 agents use custom gateway models?</summary>

48 support gateway configuration or native-app integration. Cursor CLI, Copilot CLI, and Devin manage vendor-native models through vendor accounts. Native applications and terminal launches have specific prerequisites; see the [Agent contracts](docs/agent-integration-contracts.md).

</details>

<details>
<summary>How do I use the panel on a remote Linux machine?</summary>

Extract the release archive and run `./codex-mixin panel`. Keep the process running and open the printed address using a reachable host address. The panel listens on all interfaces on port 8318 (falling back to a free port in 8000–9000 if taken) and requires the token generated at startup. See [Installation](https://github.com/Edward-lyz/codex-mixin/wiki/Installation) and [Data and Security](https://github.com/Edward-lyz/codex-mixin/wiki/Data-and-Security).

</details>

<details>
<summary>Does the context engine compress every request?</summary>

It is off by default. When enabled, it requires a session ID and a supported request path: Responses and mappable native Anthropic Messages content can enter the shared loop; hosted tools and native content that cannot be mapped faithfully pass through. See [Context engine](docs/context-engine.md).

</details>

<details>
<summary>Are exported backups encrypted?</summary>

No. Backups contain API keys and use Base64 encoding. Treat them as credentials. See [Configuration backup and restore](https://github.com/Edward-lyz/codex-mixin/wiki/Configuration-Backup-and-Restore).

</details>

---

## License

Codex Mixin is licensed under [CC BY-NC 4.0](LICENSE): you may use, modify and redistribute it with attribution, but not for commercial purposes. Contact the author for commercial licensing. Third-party notices are in [NOTICE](NOTICE).
