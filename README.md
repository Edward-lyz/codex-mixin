<p align="center">
  <img src="docs/assets/app-icon.png" width="96" alt="Codex Mixin Logo">
</p>

<h1 align="center">Codex Mixin</h1>

<p align="center">
  <b>The Local AI Gateway and Desktop Control Plane for Official Codex and 50+ Coding Agents</b><br>
  <sub>Route any LLM into official OpenAI Codex while preserving your ChatGPT accounts, GPT models, and native workflow.</sub>
</p>

<p align="center">
  <a href="README_zh.md">简体中文</a> | <b>English</b>
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
  <a href="#core-values">Core Values</a> ·
  <a href="#recent-updates">What's New</a> ·
  <a href="#showcase">Showcase</a> ·
  <a href="#product-surfaces">Product Surfaces</a> ·
  <a href="#providers-and-protocols">Providers</a> ·
  <a href="#agent-integrations">Agent Integrations</a> ·
  <a href="#advanced-features">Advanced Features</a> ·
  <a href="#quick-start">Quick Start</a> ·
  <a href="https://github.com/Edward-lyz/codex-mixin/wiki">Wiki</a>
</p>

---

## Overview

Codex Mixin is a high-performance local AI gateway and cross-platform desktop control plane written in Rust.

It bridges third-party language models—such as DeepSeek, Claude, Gemini, Baidu OneAPI, AWS Bedrock, Google Antigravity, and any OpenAI/Anthropic-compatible endpoint—directly into the official OpenAI Codex desktop app and CLI. Developers can use custom models at cost while keeping official ChatGPT/OpenAI account authentication, native GPT models, conversation history, and mobile remote control fully functional.

Beyond Codex, Codex Mixin serves as a unified local routing hub and usage control center for 50 popular AI coding agents, supporting seamless gateway configuration injection and zero-trace restoration.

---

## Core Values

<table>
  <tr>
    <td width="50%">
      <b>Zero-Friction Codex Coexistence</b><br>
      Custom models and official OpenAI models coexist side by side in the Codex picker. Official model definitions sync automatically from OpenAI's repository every 30 seconds. Connecting and disconnecting preserves your existing sessions, databases, and account state.
    </td>
    <td width="50%">
      <b>Smart ECH Relay for Official GPT</b><br>
      Resolves IP addresses and Encrypted Client Hello (ECH) parameters through relay DoH. Directly connects to OpenAI endpoints across network boundaries while minimizing account risk flags.
    </td>
  </tr>
  <tr>
    <td width="50%">
      <b>Lossless Protocol Bridge</b><br>
      Translates bi-directionally between OpenAI Responses, Chat Completions, and Anthropic Messages with zero semantic loss, preserving streaming thinking traces, multi-step tool calls, and structured outputs.
    </td>
    <td width="50%">
      <b>Universal Hub for 50+ Agents</b><br>
      Catalog covers 50 coding tools (Claude Code, OpenCode, Copilot, Cline, and more). The default build supports 38 one-click configuration adapters with clean rollback on exit.
    </td>
  </tr>
</table>

---

## Recent Updates

### What's New in v0.7.3 & Recent Releases

- **Modern Web Control Panel (`codex-mixin panel`)**: Launch a responsive Notion-style browser console designed for Linux hosts, remote development servers, and headless SSH sessions. Manage models, inspect agent connection status, and monitor token/quota usage.
- **Native Windows WinUI 3 Desktop App**: Rebuilt the Windows frontend from scratch with native WinUI 3 and Fluent Design, replacing the legacy wrapper with sub-second tray response and minimal memory overhead.
- **Model Capability Probing**: Proactively test and verify live model capabilities (Reasoning thoughts, Function Calling, Tool Search, and Web Search) with badges displayed in the UI.
- **Google Antigravity (AGY) & Native Gemini Support**: Direct Google OAuth 2.0 authentication, dynamic discovery of AGY-specific agent models and quotas, and wire protocol conversion for Gemini Developer API.
- **Agent Catalog Expanded to 50**: Added 8 configuration adapters (VSCodium, Reasonix, Command Code, fx, Empryo, Ante, MiniMax Code, Cline) across IDE extensions, CLI agents, and developer runtimes.
- **Standalone Codex Config Injection**: Codex integration writes necessary provider and model settings cleanly without requiring pre-installed CLI binaries, activating the modern `STEPS_COMMANDS` coding surface.
- **Core Performance Optimizations**: Negotiated gzip streaming for large contexts, offloaded serialization from the primary async loop, and cached capability probe results to prevent redundant provider billing.

---

## Showcase

<table>
  <tr>
    <td width="50%" align="center">
      <a href="docs/assets/APP-model-picker.png"><img src="docs/assets/APP-model-picker.png" alt="macOS Menu Bar and Model Picker"></a><br>
      <sub><b>macOS Experience</b> · Menu bar status, model catalog, and live benchmarks</sub>
    </td>
    <td width="50%" align="center">
      <a href="docs/assets/APP-ProviderModelList.png"><img src="docs/assets/APP-ProviderModelList.png" alt="Provider Discovery and Model Capability"></a><br>
      <sub><b>Model Management</b> · Automatic discovery, capability badges, and latency benchmarks</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <a href="docs/assets/fusion-review.png"><img src="docs/assets/fusion-review.png" alt="Interactive Model Fusion inside Codex"></a><br>
      <sub><b>Model Fusion</b> · Multi-model orchestration and Judge review inside Codex</sub>
    </td>
    <td width="50%" align="center">
      <a href="docs/assets/Mobile_Choice.PNG"><img src="docs/assets/Mobile_Choice.PNG" width="260" alt="Select Mixin Models from Mobile"></a><br>
      <sub><b>Mobile & Remote Tasks</b> · Select custom gateway models from mobile Codex</sub>
    </td>
  </tr>
</table>

<p align="center">
  <a href="https://github.com/Edward-lyz/codex-mixin/wiki/Product-Tour">View the complete Product Tour with macOS, WinUI 3, Web Panel, and Mobile Walkthroughs</a>
</p>

---

## Product Surfaces

| Component | Distribution | Core Capabilities |
| --- | --- | --- |
| **macOS App** | Native Menu Bar App | Live status in menu bar, model management, capability testing, benchmark suite, Base64 backup/restore, auto-repair. |
| **Windows App** | Native WinUI 3 App | Fluent Design interface, system tray service, provider settings, agent connection management, and log viewer. |
| **Web Control Panel** | `codex-mixin panel` | Modern Notion-style responsive web console, ideal for Linux servers, cloud VMs, SSH terminals, and remote setups. |
| **CLI Core** | Single Rust Binary | Cross-platform core runtime with deterministic subcommands, JSON contracts, and background daemon lifecycle controls. |

---

## Providers and Protocols

| Provider / Channel | Primary Protocols | Authentication | Key Features |
| --- | --- | --- | --- |
| **Baidu OneAPI** | Anthropic Messages / OpenAI Responses | API Key / Tenant Token | Full support for GLM-5.3, Claude, DeepSeek; maintains session affinity automatically. |
| **OpenRouter** | OpenAI Responses / Chat Completions | Bearer Token | Access global open/closed models with context compression and streaming pass-through. |
| **DeepSeek** | OpenAI Responses / Chat Completions | API Key | Native DeepSeek-V3 and DeepSeek-R1 streaming reasoning traces and tool calls. |
| **Google Antigravity (AGY)** | Gemini Developer API / Google Native | Google OAuth 2.0 | Dynamic AGY agent model discovery, real-time quota tracking, instruction stream verification. |
| **OpenCode Go** | OpenAI Responses | Session Token | Auto-injects `x-opencode-session` header; stable UUID mapping for multi-turn sessions. |
| **AWS Bedrock** | Anthropic Messages | AWS SigV4 Signer | Claude 3.5 / 3.7 families, cross-region endpoints, and model-level API path overrides. |
| **Generic Endpoints** | OpenAI / Anthropic Compatible | Custom Header / Key | Add any compatible custom endpoint with capability probing and latency testing. |

---

## Agent Integrations

The catalog covers **50 AI Coding Agents**. The default build provides **38 one-click gateway configuration adapters**.

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
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/opencode-dark.svg">
        <img src="docs/assets/agents/opencode-light.svg" width="36" height="36" alt="OpenCode logo">
      </picture><br>
      <sub>OpenCode</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/pi-dark.svg">
        <img src="docs/assets/agents/pi-light.svg" width="36" height="36" alt="Pi logo">
      </picture><br>
      <sub>Pi</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/openchamber.svg" width="36" height="36" alt="OpenChamber logo"><br>
      <sub>OpenChamber</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/mimocode-dark.svg">
        <img src="docs/assets/agents/mimocode-light.svg" width="36" height="36" alt="MiMo Code logo">
      </picture><br>
      <sub>MiMo Code</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/omo-dark.svg">
        <img src="docs/assets/agents/omo-light.svg" width="36" height="36" alt="OmO logo">
      </picture><br>
      <sub>OmO</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/goose-dark.svg">
        <img src="docs/assets/agents/goose-light.svg" width="36" height="36" alt="Goose logo">
      </picture><br>
      <sub>Goose</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/vscode-dark.svg">
        <img src="docs/assets/agents/vscode-light.svg" width="36" height="36" alt="VS Code logo">
      </picture><br>
      <sub>VS Code</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/vscode-insiders-dark.svg">
        <img src="docs/assets/agents/vscode-insiders-light.svg" width="36" height="36" alt="VS Code Insiders logo">
      </picture><br>
      <sub>VS Code Insiders</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/copilot-jetbrains-dark.svg">
        <img src="docs/assets/agents/copilot-jetbrains-light.svg" width="36" height="36" alt="Copilot for JetBrains logo">
      </picture><br>
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
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/omp-dark.svg">
        <img src="docs/assets/agents/omp-light.svg" width="36" height="36" alt="Oh My Pi logo">
      </picture><br>
      <sub>Oh My Pi</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/hermes-dark.svg">
        <img src="docs/assets/agents/hermes-light.svg" width="36" height="36" alt="Hermes Agent logo">
      </picture><br>
      <sub>Hermes Agent</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/morph.svg" width="36" height="36" alt="Mister Morph logo"><br>
      <sub>Mister Morph</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/kimi-dark.svg">
        <img src="docs/assets/agents/kimi-light.svg" width="36" height="36" alt="Kimi Code logo">
      </picture><br>
      <sub>Kimi Code</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/qwen.svg" width="36" height="36" alt="Qwen Code logo"><br>
      <sub>Qwen Code</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/droid-dark.svg">
        <img src="docs/assets/agents/droid-light.svg" width="36" height="36" alt="Droid logo">
      </picture><br>
      <sub>Droid</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/qoder-dark.svg">
        <img src="docs/assets/agents/qoder-light.svg" width="36" height="36" alt="Qoder logo">
      </picture><br>
      <sub>Qoder</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/qoder-cn-dark.svg">
        <img src="docs/assets/agents/qoder-cn-light.svg" width="36" height="36" alt="Qoder CN logo">
      </picture><br>
      <sub>Qoder CN</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/grok-dark.svg">
        <img src="docs/assets/agents/grok-light.svg" width="36" height="36" alt="Grok Build logo">
      </picture><br>
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
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/vscodium-dark.svg">
        <img src="docs/assets/agents/vscodium-light.svg" width="36" height="36" alt="VSCodium logo">
      </picture><br>
      <sub>VSCodium</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/reasonix.svg" width="36" height="36" alt="Reasonix logo"><br>
      <sub>Reasonix</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/commandcode-dark.svg">
        <img src="docs/assets/agents/commandcode-light.svg" width="36" height="36" alt="Command Code logo">
      </picture><br>
      <sub>Command Code</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/fx-dark.svg">
        <img src="docs/assets/agents/fx-light.svg" width="36" height="36" alt="fx logo">
      </picture><br>
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
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/cline-dark.svg">
        <img src="docs/assets/agents/cline-light.svg" width="36" height="36" alt="Cline logo">
      </picture><br>
      <sub>Cline</sub>
    </td>
    <td width="20%" align="center"></td>
    <td width="20%" align="center"></td>
  </tr>
</table>

### Integration Unavailable (12)

The following 12 tools rely on OS keychains, vendor-proprietary launchers, encrypted credential stores, or running runtime APIs without supported configuration injection hooks:

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
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/cursor-dark.svg">
        <img src="docs/assets/agents/cursor-light.svg" width="36" height="36" alt="Cursor CLI logo">
      </picture><br>
      <sub>Cursor CLI</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/cursor-local-dark.svg">
        <img src="docs/assets/agents/cursor-local-light.svg" width="36" height="36" alt="Cursor logo">
      </picture><br>
      <sub>Cursor</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/zed-dark.svg">
        <img src="docs/assets/agents/zed-light.svg" width="36" height="36" alt="Zed logo">
      </picture><br>
      <sub>Zed</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/copilot-dark.svg">
        <img src="docs/assets/agents/copilot-light.svg" width="36" height="36" alt="Copilot CLI logo">
      </picture><br>
      <sub>Copilot CLI</sub>
    </td>
    <td width="20%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/agents/devin-dark.svg">
        <img src="docs/assets/agents/devin-light.svg" width="36" height="36" alt="Devin logo">
      </picture><br>
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

> Connect or restore configurations via **Agent Management** in the desktop apps, or run CLI commands: `codex-mixin agent connect <id>` and `codex-mixin agent disconnect <id>`. For technical boundaries, schemas, and contract details, see [Agent integration contracts](docs/agent-integration-contracts.md); for logo attributions, see [Agent Logos Attribution](macos/assets/agents/ATTRIBUTION.md).

---

## Advanced Features

### 1. Model Fusion & Time-Based Routing

- **Multi-Model Orchestration**: Dispatch complex tasks to primary and auxiliary models simultaneously. A configured Judge model evaluates candidate outputs to ensure robust code generation.
- **Time-Based Peak / Off-Peak Routing**: Automatically switch upstream routes under a single model identity based on time schedules (for example, routing to high-concurrency private endpoints during working hours and low-cost public endpoints off-peak).

### 2. Native Claude Desktop Integration

Route sessions from the Code tab in official [Claude Desktop](https://claude.com/download) through your local gateway:

```bash
# Connect Claude Desktop
codex-mixin agent connect claude-desktop

# Disconnect and restore original configuration
codex-mixin agent disconnect claude-desktop
```

Every selected catalog model receives a stable `claude-sonnet-mixin-*` routing alias with full 1M context indicator flags while preserving your official Claude.ai login state.

### 3. Portability & Security

- **Encrypted Base64 Profile Backup**: Export all provider configurations and keys into an encrypted Base64 string for instant atomic import on another workstation.
- **Optional Client Authentication (`client-auth`)**: Default builds accept local requests without inbound gateway tokens. Compile with `--features client-auth` to enforce strict client keys and loopback-only Codex OAuth verification.
- **Dynamic Port Binding**: Gateways and web panels automatically allocate random available ports within `8000–9000` to prevent collisions, generating per-start authorization tokens.

---

## Quick Start

### 1. Installation

- **macOS**: Download the `.dmg` installer from [Releases](https://github.com/Edward-lyz/codex-mixin/releases/latest) and drag to Applications.
- **Windows**: Download the WinUI 3 installer or portable `.zip` from [Releases](https://github.com/Edward-lyz/codex-mixin/releases/latest).
- **Linux / Source**:
  ```bash
  git clone https://github.com/Edward-lyz/codex-mixin.git
  cd codex-mixin
  cargo build --locked --release
  ```

### 2. Common CLI Commands

```bash
# Start background gateway daemon
codex-mixin start

# Open the modern web control panel in your browser
codex-mixin panel

# Connect official Codex while preserving official account & GPT models
codex-mixin agent connect codex --mode official-account

# Connect Claude Code CLI
codex-mixin agent connect claude

# Restore clients to default state
codex-mixin agent disconnect codex
codex-mixin agent disconnect claude

# Probe capabilities of saved models for a provider
codex-mixin --no-tui provider probe <provider-id>
```

---

## Documentation

Comprehensive guides and operational reference are maintained in the [GitHub Wiki](https://github.com/Edward-lyz/codex-mixin/wiki):

- [Product Tour](https://github.com/Edward-lyz/codex-mixin/wiki/Product-Tour)
- [Installation](https://github.com/Edward-lyz/codex-mixin/wiki/Installation) & [Quick Start](https://github.com/Edward-lyz/codex-mixin/wiki/Quick-Start)
- [Configuration Backup & Restore](https://github.com/Edward-lyz/codex-mixin/wiki/Configuration-Backup-and-Restore)
- [Providers & Models](https://github.com/Edward-lyz/codex-mixin/wiki/Providers-and-Models)
- [Client & Agent Integrations](https://github.com/Edward-lyz/codex-mixin/wiki/Client-Integrations)
- [Official Model Catalog & ECH Direct Access](https://github.com/Edward-lyz/codex-mixin/wiki/ECH-GPT-Access)
- [CLI Reference Manual](https://github.com/Edward-lyz/codex-mixin/wiki/CLI-Reference)
- [Troubleshooting](https://github.com/Edward-lyz/codex-mixin/wiki/Troubleshooting) & [FAQ](https://github.com/Edward-lyz/codex-mixin/wiki/FAQ)

---

## License

This project is licensed under [LICENSE](LICENSE) and [NOTICE](NOTICE).
