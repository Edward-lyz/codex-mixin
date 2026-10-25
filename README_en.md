<p align="center">
  <img src="docs/assets/app-icon.png" width="96" alt="Codex Mixin Logo">
</p>

<h1 align="center">Codex Mixin</h1>

<p align="center">
  <b>A local model gateway for official OpenAI Codex and popular AI coding agents</b><br>
  <sub>Use third-party model providers while you keep your official account, GPT models, and remote tasks.</sub>
</p>

<p align="center">
  <a href="README.md">简体中文</a> | <b>English</b>
</p>

<p align="center">
  <a href="https://github.com/Edward-lyz/codex-mixin/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/Edward-lyz/codex-mixin?color=blue&label=version"></a>
  <a href="https://github.com/Edward-lyz/codex-mixin/releases"><img alt="Platforms" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey"></a>
  <a href="https://www.rust-lang.org/"><img alt="Rust Edition" src="https://img.shields.io/badge/rust-2024%20edition-orange"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Source%20Code%20Viewing%201.0-green"></a>
  <a href="https://github.com/Edward-lyz/codex-mixin/wiki"><img alt="Docs" src="https://img.shields.io/badge/docs-GitHub%20Wiki-blueviolet"></a>
</p>

<p align="center">
  <a href="#overview">Overview</a> ·
  <a href="#quick-start">Quick Start</a> ·
  <a href="#entry-points">Entry Points</a> ·
  <a href="#interface">Interface</a> ·
  <a href="#features">Features</a> ·
  <a href="#providers">Providers</a> ·
  <a href="#agent-integrations">Agents</a> ·
  <a href="#documentation">Docs</a>
</p>

---

## Overview

Codex Mixin is a local gateway written in Rust. It provides OpenAI-, Anthropic-, and Gemini-compatible endpoints on your machine and routes requests from official Codex and other coding agents to the model providers you configure: DeepSeek, Claude, Gemini, Baidu OneAPI, AWS Bedrock, Google Antigravity, or any OpenAI- or Anthropic-compatible endpoint.

When you connect official Codex, your custom models appear in the model picker next to the official GPT models. ChatGPT sign-in, session history, and mobile remote tasks continue to work. When you disconnect, the gateway restores the client's original configuration.

Start with the graphical interface: the macOS menu bar, the Windows system tray, or the Web control panel for Linux and remote machines. Add providers, select models, connect Agents, and view usage in the interface. The catalog lists 50 Agents; default release builds support configuration and restore for 38 of them.

---

## Quick Start

### 1. Download and open

Choose your platform and architecture from [Releases](https://github.com/Edward-lyz/codex-mixin/releases/latest). You do not need Rust or a source build.

| Platform | Download and run | Graphical entry |
| --- | --- | --- |
| macOS | Download the Apple Silicon or Intel `.dmg`, drag the app to Applications, and launch it | Menu bar icon → Open Console |
| Windows | Run `windows-x86_64-setup.exe`, or extract and launch the portable Zip | System tray icon → Open Console |
| Linux | Download the x86_64 or ARM64 `unknown-linux-musl.tar.gz`, extract it, and run `./codex-mixin panel`; a `.deb` is also available | Open the address printed in the terminal in your browser |

The Linux archive contains a ready-to-run binary. Run it from the extracted directory and keep the panel process running while you use it. See [Installation](https://github.com/Edward-lyz/codex-mixin/wiki/Installation) for system requirements, file names, and remote access.

### 2. Configure models and connect an Agent

1. In Models & Services, add a provider with an API key or OAuth sign-in.
2. Refresh the model list, select models, save, and make sure the gateway is running.
3. Open the Agents page and connect an installed client. For Codex, choose Official Account mode to keep official GPT models and ChatGPT sign-in.
4. Restart the client or open a new session, then select a custom model.

### 3. Check status and usage

Use the menu bar or tray for gateway status and provider quota. Open Usage in the console for daily and per-reply Tokens, cache results, and speed. Disconnect an Agent to restore its original configuration.

The Web panel listens on all interfaces on a random port from 8000 to 9000 and requires a token generated for each panel start. For remote access, use the reachable host address and the actual panel port. See [Data and Security](https://github.com/Edward-lyz/codex-mixin/wiki/Data-and-Security) for gateway and panel access boundaries.

---

## Entry Points

| Entry | Main use |
| --- | --- |
| macOS menu bar / Windows tray | Gateway controls, quota, and console access; advanced menus for Fusion and backups |
| Desktop console | Models & Services, Agents, and Usage |
| Web control panel | Models & Services, Agents, Usage, Fusion, and advanced settings in a browser; suitable for Linux and remote machines |
| CLI | Scripts, automation, and diagnostics; see [CLI Reference](https://github.com/Edward-lyz/codex-mixin/wiki/CLI-Reference) |

---

## Interface

The screenshots below show the macOS app during the v0.7.4 release.

<p align="center">
  <a href="docs/assets/APP-Console-Models-v074.png"><img src="docs/assets/APP-Console-Models-v074.png" alt="Models and Services in the macOS console"></a><br>
  <sub><b>Models & Services</b> · Providers, model selection, capabilities, and context settings</sub>
</p>

<table>
  <tr>
    <td width="50%" align="center">
      <a href="docs/assets/APP-Console-Agents-v074.png"><img src="docs/assets/APP-Console-Agents-v074.png" alt="Agents page in the macOS console"></a><br>
      <sub><b>Agents</b> · Installed versions, connection state, and restore controls</sub>
    </td>
    <td width="50%" align="center">
      <a href="docs/assets/APP-Console-Usage-v074.png"><img src="docs/assets/APP-Console-Usage-v074.png" alt="Usage page in the macOS console"></a><br>
      <sub><b>Usage</b> · Daily totals, per-reply records, cache results, and speed</sub>
    </td>
  </tr>
</table>

Native Windows WinUI 3 interface (current development build): check quota from the tray, and manage models, agents, usage, and context in the console.

<p align="center">
  <a href="docs/assets/Windows-Tray.png"><img src="docs/assets/Windows-Tray.png" width="360" alt="Windows light-mode tray showing gateway status and quota rings"></a>
</p>

<p align="center">
  <a href="docs/assets/Windows-Console-Models.png"><img src="docs/assets/Windows-Console-Models.png" alt="Models and Services in the Windows console"></a>
</p>

See the [Product Tour](https://github.com/Edward-lyz/codex-mixin/wiki/Product-Tour) for interface details. Models and statistics reflect the state at capture time.

---

## Features

**Works with official Codex.** Custom models join the Codex model catalog under their own names and appear next to the official models. The official catalog comes from OpenAI. Connecting and disconnecting do not change the Codex session database or account state.

**Protocol translation.** The gateway translates requests and streamed responses between OpenAI Responses, Chat Completions, Anthropic Messages, and Gemini. It keeps reasoning content, tool calls, and multi-turn context.

**Capability probing.** The gateway sends real requests to the models you add and records Reasoning, Function Calling, Tool Search, and Web Search support. The catalog and Fusion use the cached results.

**Usage and quota.** View daily and per-reply Tokens, cache hit ratio, reasoning usage, returned model, time to first token, and output speed. The latest 5000 replies are kept locally. Provider quota supports rings or bars, with red indicators above 90% used.

**Model fusion.** One model ID can run a primary model and helper models, and a judge model reviews the results. One model ID can also route to different providers at different times of day.

**Claude Desktop.** Connect [Claude Desktop](https://claude.com/download) on the Agents page to use selected gateway models in its Code tab while keeping Claude.ai sign-in. Disconnect to restore the original configuration.

**Configuration backup.** Use the advanced menus to export or import providers, models, and Fusion settings for recovery or migration. Backups contain API keys. Base64 is encoding, so protect the backup as a credential.

**ECH access to official GPT (experimental).** Enable it from the advanced menu to test DNS and TLS before connecting to official OpenAI endpoints through DoH and ECH. See [ECH GPT](docs/ech-gpt.md).

**Client authentication (optional).** The default build does not check an inbound key, so local clients can use the gateway directly. A build with `--features client-auth` requires clients to send a key.

---

## Providers

Choose one of these presets when adding a provider in the interface. Use Custom for other compatible endpoints.

| Preset | Provider | Upstream protocol | Authentication |
| --- | --- | --- | --- |
| `openai` | OpenAI | OpenAI Responses | ChatGPT OAuth |
| `baidu-oneapi` | Baidu OneAPI | Anthropic Messages; GPT models use OpenAI Responses | API key |
| `openrouter` | OpenRouter | OpenAI Chat Completions | API key |
| `deepseek` | DeepSeek | OpenAI Responses | API key |
| `agy` | Google Antigravity | Gemini | Google OAuth |
| `opencode-go` | OpenCode Go | OpenAI Responses | API key, with a session affinity header |
| `aws-bedrock` | Amazon Bedrock (Mantle) | Anthropic Messages | Bedrock API key or AWS access key |
| `custom` | Any compatible endpoint | OpenAI Responses (default) | Custom headers and key |

Test the connection and refresh models in the interface. Use the model action menu to probe capabilities again; probes send real requests and can consume quota.

---

## Agent Integrations

The catalog lists 50 AI coding agents. In the default build, 38 of them can be connected to the gateway with one command and restored to their original configuration on disconnect.

### Supported Integrations (38)

<table>
  <tr>
    <td width="20%" align="center">
      <img src="macos/assets/agents/codex.svg" width="36" height="36" alt="Codex logo"><br>
      <sub>Codex</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/claude.svg" width="36" height="36" alt="Claude Code logo"><br>
      <sub>Claude Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/claude-desktop.svg" width="36" height="36" alt="Claude Desktop logo"><br>
      <sub>Claude Desktop</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/dsh.svg" width="36" height="36" alt="DSH logo"><br>
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
      <img src="macos/assets/agents/openchamber.svg" width="36" height="36" alt="OpenChamber logo"><br>
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
      <img src="macos/assets/agents/air.svg" width="36" height="36" alt="Air logo"><br>
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
      <img src="macos/assets/agents/morph.svg" width="36" height="36" alt="Mister Morph logo"><br>
      <sub>Mister Morph</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/kimi-light.svg#gh-light-mode-only" width="36" height="36" alt="Kimi Code logo"><img src="docs/assets/agents/kimi-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Kimi Code logo"><br>
      <sub>Kimi Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/qwen.svg" width="36" height="36" alt="Qwen Code logo"><br>
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
      <img src="macos/assets/agents/workbuddy.svg" width="36" height="36" alt="WorkBuddy logo"><br>
      <sub>WorkBuddy</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/codebuddy.svg" width="36" height="36" alt="CodeBuddy logo"><br>
      <sub>CodeBuddy</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/pencil.svg" width="36" height="36" alt="Pencil logo"><br>
      <sub>Pencil</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/t3code.svg" width="36" height="36" alt="T3 Code logo"><br>
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
      <img src="macos/assets/agents/reasonix.svg" width="36" height="36" alt="Reasonix logo"><br>
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
      <img src="macos/assets/agents/empryo.svg" width="36" height="36" alt="Empryo logo"><br>
      <sub>Empryo</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="macos/assets/agents/ante.svg" width="36" height="36" alt="Ante logo"><br>
      <sub>Ante</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/minimax-code.svg" width="36" height="36" alt="MiniMax Code logo"><br>
      <sub>MiniMax Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/cline-light.svg#gh-light-mode-only" width="36" height="36" alt="Cline logo"><img src="docs/assets/agents/cline-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Cline logo"><br>
      <sub>Cline</sub>
    </td>
    <td width="20%" align="center"></td>
    <td width="20%" align="center"></td>
  </tr>
</table>

### Integration Unavailable (12)

The following 12 tools store their configuration behind OS keychain encryption, process-level injection, closed authentication protocols, or private in-app runtime APIs. Codex Mixin does not write gateway configuration for them:

<table>
  <tr>
    <td width="20%" align="center">
      <img src="macos/assets/agents/gemini.svg" width="36" height="36" alt="Gemini CLI logo"><br>
      <sub>Gemini CLI</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/agy.svg" width="36" height="36" alt="agy logo"><br>
      <sub>agy</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/aside.svg" width="36" height="36" alt="Aside logo"><br>
      <sub>Aside</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/cursor-light.svg#gh-light-mode-only" width="36" height="36" alt="Cursor CLI logo"><img src="docs/assets/agents/cursor-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Cursor CLI logo"><br>
      <sub>Cursor CLI</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/cursor-local-light.svg#gh-light-mode-only" width="36" height="36" alt="Cursor logo"><img src="docs/assets/agents/cursor-local-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Cursor logo"><br>
      <sub>Cursor</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="docs/assets/agents/zed-light.svg#gh-light-mode-only" width="36" height="36" alt="Zed logo"><img src="docs/assets/agents/zed-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Zed logo"><br>
      <sub>Zed</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/copilot-light.svg#gh-light-mode-only" width="36" height="36" alt="Copilot CLI logo"><img src="docs/assets/agents/copilot-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Copilot CLI logo"><br>
      <sub>Copilot CLI</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/devin-light.svg#gh-light-mode-only" width="36" height="36" alt="Devin logo"><img src="docs/assets/agents/devin-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Devin logo"><br>
      <sub>Devin</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/muse.svg" width="36" height="36" alt="Muse logo"><br>
      <sub>Muse</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/hanako.svg" width="36" height="36" alt="Hanako logo"><br>
      <sub>Hanako</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="docs/assets/agents/alma.svg" width="36" height="36" alt="Alma logo"><br>
      <sub>Alma</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/cindy.svg" width="36" height="36" alt="Cindy logo"><br>
      <sub>Cindy</sub>
    </td>
    <td width="20%" align="center"></td>
    <td width="20%" align="center"></td>
    <td width="20%" align="center"></td>
  </tr>
</table>

> Connect or restore configurations on the Agent page of the console, or run CLI commands: `codex-mixin agent connect <id>` and `codex-mixin agent disconnect <id>`. For technical boundaries, schemas, and contract details, see [Agent integration contracts](docs/agent-integration-contracts.md); for logo attributions, see [Agent Logos Attribution](macos/assets/agents/ATTRIBUTION.md).

---

## Documentation

In this repository:

- [Architecture and module boundaries](docs/architecture.md)
- [Agent integration contracts](docs/agent-integration-contracts.md)
- [Google Antigravity](docs/antigravity.md)
- [ECH access to official GPT](docs/ech-gpt.md)
- [Browser CORS allowlist](docs/cors.md)

Guides in the [GitHub Wiki](https://github.com/Edward-lyz/codex-mixin/wiki):

- [Product Tour](https://github.com/Edward-lyz/codex-mixin/wiki/Product-Tour)
- [Installation](https://github.com/Edward-lyz/codex-mixin/wiki/Installation) and [Quick Start](https://github.com/Edward-lyz/codex-mixin/wiki/Quick-Start)
- [Usage and quota](https://github.com/Edward-lyz/codex-mixin/wiki/Usage-and-Quota)
- [Configuration backup and restore](https://github.com/Edward-lyz/codex-mixin/wiki/Configuration-Backup-and-Restore)
- [Providers and models](https://github.com/Edward-lyz/codex-mixin/wiki/Providers-and-Models)
- [Client integrations](https://github.com/Edward-lyz/codex-mixin/wiki/Client-Integrations)
- [CLI reference](https://github.com/Edward-lyz/codex-mixin/wiki/CLI-Reference)
- [Troubleshooting](https://github.com/Edward-lyz/codex-mixin/wiki/Troubleshooting) and [FAQ](https://github.com/Edward-lyz/codex-mixin/wiki/FAQ)

For version changes, see [RELEASE_NOTES.md](RELEASE_NOTES.md).

---

## License

This project is released under the [LICENSE](LICENSE). Third-party notices are in [NOTICE](NOTICE).
