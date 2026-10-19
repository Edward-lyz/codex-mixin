<p align="center">
  <img src="docs/assets/app-icon.png" width="96" alt="Codex Mixin Logo">
</p>

<h1 align="center">Codex Mixin</h1>

<p align="center">
  <b>官方 OpenAI Codex 与 50+ 编程智能体的跨平台本地网关与桌面控制中心</b><br>
  <sub>在保留官方账号、GPT 原生模型与远程任务的前提下，无缝接入任意大模型与自定义供应商。</sub>
</p>

<p align="center">
  <b>简体中文</b> | <a href="README_en.md">English</a>
</p>

<p align="center">
  <a href="https://github.com/Edward-lyz/codex-mixin/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/Edward-lyz/codex-mixin?color=blue&label=version"></a>
  <a href="https://github.com/Edward-lyz/codex-mixin/releases"><img alt="Platforms" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey"></a>
  <a href="https://www.rust-lang.org/"><img alt="Rust Edition" src="https://img.shields.io/badge/rust-2024%20edition-orange"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Source%20Code%20Viewing%201.0-green"></a>
  <a href="https://github.com/Edward-lyz/codex-mixin/wiki"><img alt="Docs" src="https://img.shields.io/badge/docs-GitHub%20Wiki-blueviolet"></a>
</p>

<p align="center">
  <a href="#概览">概览</a> ·
  <a href="#核心价值">核心价值</a> ·
  <a href="#最近更新">最近更新</a> ·
  <a href="#界面展示">界面展示</a> ·
  <a href="#产品形态">产品形态</a> ·
  <a href="#供应商与协议支持">供应商与协议</a> ·
  <a href="#agent-接入生态">Agent 接入</a> ·
  <a href="#进阶特性">进阶特性</a> ·
  <a href="#快速开始">快速开始</a> ·
  <a href="https://github.com/Edward-lyz/codex-mixin/wiki">完整文档</a>
</p>

---

## 概览

Codex Mixin 是一个基于 Rust 构建的高性能跨平台本地网关与桌面控制中心。

它打破了官方 OpenAI Codex 对底层模型的局限，让开发者可以在官方 Codex 客户端与 CLI 中自由选用 DeepSeek、Claude、Gemini、Baidu OneAPI、AWS Bedrock、Google Antigravity 及任意兼容 OpenAI / Anthropic 协议的模型；同时完整保留官方 ChatGPT/OpenAI 账号认证、GPT 原生模型、历史会话与移动端远程任务管理。

不仅面向 Codex，Codex Mixin 还是一体化的本地模型路由器与用量治理中枢，内置支持 50 款主流 AI Coding Agent 的配置接管与一键撤销恢复。

---

## 核心价值

<table>
  <tr>
    <td width="50%">
      <b>官方 Codex 无感共存</b><br>
      自定义模型与 OpenAI 官方模型在 Codex 选择器中共存并重名隔离。官方模型目录每 30 秒自动同步 OpenAI 最新列表，接入与断开均不损坏原有会话、数据库与账号状态。
    </td>
    <td width="50%">
      <b>官方 GPT ECH 智能直连</b><br>
      基于中继 DoH 与 ECH (Encrypted Client Hello) 技术，在受限网络环境下自动优选 IP 节点直连 OpenAI 官方端点，避免风控封号与网络中断。
    </td>
  </tr>
  <tr>
    <td width="50%">
      <b>全双工协议无损互转</b><br>
      零损耗桥接 OpenAI Responses、Chat Completions 与 Anthropic Messages 协议，深度支持流式思考过程 (Thinking)、复杂工具调用 (Tool Calls) 与多轮上下文。
    </td>
    <td width="50%">
      <b>50+ 编程智能体统一纳管</b><br>
      管理目录覆盖 50 款主流 Agent（如 Claude Code、OpenCode、Copilot、Cline 等），默认构建支持 38 款一键写入网关配置并在退出时无痕回退。
    </td>
  </tr>
</table>

---

## 最近更新

### v0.7.3 & 近期版本演进

- **全新 Notion 风格 Web 控制面板**：运行 `codex-mixin panel` 即可打开现代化响应式浏览器控制台，专为 Linux、远端开发机和 SSH 终端设计，完整管理模型目录、Agent 接入与用量监控。
- **Windows 原生 WinUI 3 客户端重构**：全面重构 Windows 桌面壳，基于现代 WinUI 3 与 Fluent Design 开发，替代旧版方案，系统托盘毫秒级响应，体积更小、更省资源。
- **模型真实能力主动探测 (Capability Probing)**：模型管理中支持一键对已添加模型执行真实探测，自动化识别并标注 Reasoning 思考能力、Function Calling 工具调用、Tool Search 检索与 Web Search 联网搜索标签。
- **Google Antigravity (AGY) & Gemini 协议原生接入**：支持原生 Google OAuth 凭证，动态发现 AGY 专属模型与实时额度，底层适配 Gemini Developer API。
- **Agent 生态扩展至 50 款**：新增接入 VSCodium、Reasonix、Command Code、fx、Empryo、Ante、MiniMax Code、Cline 等 8 个适配器，覆盖主流 IDE、插件与 CLI。
- **Codex 独立配置注入架构**：优化配置注入逻辑，无需本地预装或启动 Codex CLI 即可完成原子化写入，桌面端自动切换至 `STEPS_COMMANDS` 现代编码视图。
- **传输性能深度优化**：支持大文本协商 gzip 压缩，序列化与临时流控移出核心异步运行时，降低显存与内存开销；支持模型能力缓存复用，避免重复计费。

---

## 界面展示

<table>
  <tr>
    <td width="50%" align="center">
      <a href="docs/assets/APP-model-picker.png"><img src="docs/assets/APP-model-picker.png" alt="macOS 菜单栏与模型选择器"></a><br>
      <sub><b>macOS 原生体验</b> · 菜单栏状态、模型目录与基准测速</sub>
    </td>
    <td width="50%" align="center">
      <a href="docs/assets/APP-ProviderModelList.png"><img src="docs/assets/APP-ProviderModelList.png" alt="Provider 模型发现与能力管理"></a><br>
      <sub><b>模型与能力管理</b> · 自动发现、能力状态识别与延迟测速</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <a href="docs/assets/fusion-review.png"><img src="docs/assets/fusion-review.png" alt="Codex 内嵌模型融合与 Judge 评审"></a><br>
      <sub><b>模型融合 (Fusion)</b> · 嵌入官方 Codex 内部的多模型编排与 Judge 评审</sub>
    </td>
    <td width="50%" align="center">
      <a href="docs/assets/Mobile_Choice.PNG"><img src="docs/assets/Mobile_Choice.PNG" width="260" alt="移动端选择 Mixin 模型"></a><br>
      <sub><b>移动端与远程控制</b> · 在移动端 Codex 中直接调用本地网关模型</sub>
    </td>
  </tr>
</table>

<p align="center">
  <a href="https://github.com/Edward-lyz/codex-mixin/wiki/Product-Tour">查看包含完整 macOS、Windows WinUI 3、Web Panel 和移动端全景的 Product Tour</a>
</p>

---

## 产品形态

| 端形态 | 交付方式 | 核心定位与能力 |
| --- | --- | --- |
| **macOS App** | 原生菜单栏 App | 菜单栏实时状态、Provider 与模型管理、能力探测、测速基准、Base64 配置备份导入导出与环境自愈。 |
| **Windows App** | 原生 WinUI 3 托盘 App | Fluent Design 原生视觉、系统托盘常驻、供应商管理、模型路由选择、一键 Agent 接管与配置恢复。 |
| **Web 控制面板** | `codex-mixin panel` 启动 | 现代 Notion 风格响应式界面，专为 Linux 主机、云端开发机、SSH 远程开发和无桌面环境设计。 |
| **CLI Core** | 单一跨平台二进制 | 统一的底层运行时，提供确定性子命令、JSON 格式通信契约、后台守护进程生命周期管理。 |

---

## 供应商与协议支持

| 供应商 / 渠道 | 支持主协议 | 鉴权方式 | 特色能力 |
| --- | --- | --- | --- |
| **Baidu OneAPI** | Anthropic Messages / OpenAI Responses | API Key / 租户令牌 | 完整支持 GLM-5.3、Claude、DeepSeek；自动保持 Session 亲和性。 |
| **OpenRouter** | OpenAI Responses / Chat Completions | Bearer Token | 全球主流开源与闭源模型聚合，支持上下文压缩与流式直推。 |
| **DeepSeek** | OpenAI Responses / Chat Completions | API Key | 深度支持 DeepSeek-V3 与 DeepSeek-R1 思考流式回传与工具调用。 |
| **Google Antigravity (AGY)** | Gemini Developer API / Google 原生协议 | Google OAuth 2.0 | 动态发现 AGY 专属 Agent 模型及实时额度，内置指令流完整性校验。 |
| **OpenCode Go** | OpenAI Responses | 专用会话令牌 | 自动注入 `x-opencode-session` 头部，多轮对话保持稳定 UUID 映射。 |
| **AWS Bedrock** | Anthropic Messages | AWS SigV4 鉴权 | 支持 Claude 3.5 / 3.7 全系列，支持跨区域端点与模型级路径定制。 |
| **通用兼容端点** | OpenAI / Anthropic 兼容协议 | 自定义 Header / Key | 任意兼容端点均可一键添加并执行能力探测与基准测试。 |

---

## Agent 接入生态

Codex Mixin 管理目录已覆盖 **50 款主流 AI Coding Agent**。默认构建提供 **38 款一键网关配置接入与恢复**。

### 已支持接入 (38 款)

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

### 暂未支持接入 (12 款)

以下 12 款客户端因依赖操作系统钥匙串加密（Keychain）、进程级专有注入器、专属闭源认证协议或官方应用内专有运行时 API，暂未提供网关配置写入，其技术边界保持公开透明：

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

> 安装对应框架后，可通过桌面端的 **Agent 管理** 界面一键连接或恢复配置，亦可通过 CLI 命令操作：`codex-mixin agent connect <id>` 与 `codex-mixin agent disconnect <id>`。详细各适配器文件路径、契约规范与限制见 [Agent integration contracts](docs/agent-integration-contracts.md)；图标归属声明见 [Agent Logos Attribution](macos/assets/agents/ATTRIBUTION.md)。

---

## 进阶特性

### 1. 模型融合与智能路由 (Model Fusion)

- **多模型协同编排**：支持在单次推理任务中调度主模型与辅助模型，由 Judge 模型在后台执行交叉评审，有效提升生成代码的严谨性与健壮度。
- **按时轮转与成本优化**：一个模型标识可在不同时间段自动路由到不同上游供应商（例如工作日白天路由至企业私有高性能端点，夜间及周末切换至波谷优惠公有云），实现 Token 经济效益最大化。

### 2. Claude Desktop 深度接入

通过 Codex Mixin，可直接让官方 [Claude Desktop](https://claude.com/download) 的 Code 标签页通过本地网关调度任意第三方大模型或官方 GPT：

```bash
# 接入 Claude Desktop
codex-mixin agent connect claude-desktop

# 恢复至接入前的原始状态
codex-mixin agent disconnect claude-desktop
```

每个加入的目录模型都会生成稳定的 `claude-sonnet-mixin-*` 路由别名，原生支持至少 100 万上下文窗口标识，且不干扰用户原有的 Claude.ai 账号登录。

### 3. 配置漫游与端到端安全

- **便携配置备份**：一键将所有 Provider 设置与凭据导出为经过加密的 Base64 字符串，可在多台设备间一秒导入，告别繁琐的手动配置。
- **可选客户端鉴权 (`client-auth`)**：默认构建无需携带入站 Key，方便局域网内设备共享额度；通过 `--features client-auth` 构建可开启严格的客户端密钥校验与 Codex OAuth 规则。
- **智能端口分配**：网关与 Web 面板默认在 `8000–9000` 端口段内自动随机分配可用端口，避免端口占用冲突，同时对外暴露前均打印独立一次性访问令牌。

---

## 快速开始

### 1. 安装与下载

- **macOS**：从 [Releases](https://github.com/Edward-lyz/codex-mixin/releases/latest) 下载最新 `.dmg` 文件，拖入 Applications 即可使用菜单栏 App。
- **Windows**：从 [Releases](https://github.com/Edward-lyz/codex-mixin/releases/latest) 下载安装器或免安装便携版 Zip，启动享受原生 WinUI 3 体验。
- **Linux / 源码安装**：
  ```bash
  git clone https://github.com/Edward-lyz/codex-mixin.git
  cd codex-mixin
  cargo build --locked --release
  ```

### 2. 常用操作命令

```bash
# 启动后台网关守护进程
codex-mixin start

# 启动并打开现代化 Web 控制面板
codex-mixin panel

# 将官方 Codex 接入本地网关（保留官方账号与 GPT 模型）
codex-mixin agent connect codex --mode official-account

# 将 Claude Code CLI 接入本地网关
codex-mixin agent connect claude

# 恢复指定客户端至官方默认配置
codex-mixin agent disconnect codex
codex-mixin agent disconnect claude

# 探测指定 Provider 已保存模型的能力（Reasoning/Tools/Search）
codex-mixin --no-tui provider probe <provider-id>
```

---

## 文档导航

完整安装教程、架构剖析与故障排除请参阅 [GitHub Wiki](https://github.com/Edward-lyz/codex-mixin/wiki)：

- [全景产品体验 (Product Tour)](https://github.com/Edward-lyz/codex-mixin/wiki/Product-Tour)
- [安装指南 (Installation)](https://github.com/Edward-lyz/codex-mixin/wiki/Installation) 与 [快速上手 (Quick Start)](https://github.com/Edward-lyz/codex-mixin/wiki/Quick-Start)
- [配置备份与跨机恢复](https://github.com/Edward-lyz/codex-mixin/wiki/Configuration-Backup-and-Restore)
- [供应商与模型配置 (Providers & Models)](https://github.com/Edward-lyz/codex-mixin/wiki/Providers-and-Models)
- [客户端与 Agent 接入指引](https://github.com/Edward-lyz/codex-mixin/wiki/Client-Integrations)
- [官方 GPT ECH 网络直连与 TUN 模式](https://github.com/Edward-lyz/codex-mixin/wiki/ECH-GPT-Access)
- [CLI 命令全量参考手册](https://github.com/Edward-lyz/codex-mixin/wiki/CLI-Reference)
- [故障排查 (Troubleshooting)](https://github.com/Edward-lyz/codex-mixin/wiki/Troubleshooting) 与 [常见问题 (FAQ)](https://github.com/Edward-lyz/codex-mixin/wiki/FAQ)

---

## 许可证

本项目遵循 [LICENSE](LICENSE) 与 [NOTICE](NOTICE) 协议。
