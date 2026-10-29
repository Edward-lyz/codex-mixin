<p align="center">
  <img src="docs/assets/app-icon.png" width="96" alt="Codex Mixin Logo">
</p>

<h1 align="center">Codex Mixin</h1>

<p align="center">
  <b>面向官方 OpenAI Codex 与主流 AI Coding Agent 的本地模型网关</b><br>
  <sub>在保留官方账号、GPT 模型和远程任务的同时，接入第三方模型供应商。</sub>
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
  <a href="#快速开始">快速开始</a> ·
  <a href="#使用入口">使用入口</a> ·
  <a href="#界面">界面</a> ·
  <a href="#功能">功能</a> ·
  <a href="#供应商">供应商</a> ·
  <a href="#agent-接入">Agent 接入</a> ·
  <a href="#文档">文档</a>
</p>

---

## 概览

Codex Mixin 是一个用 Rust 编写的本地网关。它在本机提供 OpenAI、Anthropic 和 Gemini 兼容接口，把官方 Codex 和其他 Coding Agent 的请求路由到你配置的模型供应商，例如 DeepSeek、Claude、Gemini、Baidu OneAPI、AWS Bedrock、Google Antigravity，以及任意 OpenAI 或 Anthropic 兼容端点。

接入官方 Codex 时，自定义模型与官方 GPT 模型同时出现在模型选择器中；ChatGPT 账号登录、历史会话和移动端远程任务保持可用。断开接入后，网关恢复客户端原来的配置。

日常使用从图形界面开始：macOS 菜单栏、Windows 系统托盘，或 Linux / 远程机器的 Web 控制面板。添加供应商、选择模型、接入 Agent 和查看用量，都可以在界面中完成。管理目录中的 50 款 Agent 均提供接入或原生模型管理：47 款支持网关配置或原生应用接入，Cursor CLI、Copilot CLI、Devin 提供厂商原生模型管理。部分应用需要保持运行或在应用内确认导入。

---

## 快速开始

### 1. 下载并打开

从 [Releases](https://github.com/Edward-lyz/codex-mixin/releases/latest) 选择对应平台和架构的发布包，无需安装 Rust 或从源码编译。

| 平台 | 下载与运行 | 图形入口 |
| --- | --- | --- |
| macOS | 下载 Apple Silicon 或 Intel 的 `.dmg`，将 App 拖入「应用程序」后启动 | 点击菜单栏图标 →「打开控制台…」 |
| Windows | 运行 `windows-x86_64-setup.exe`，或解压便携版 Zip 后启动 | 点击系统托盘图标 →「打开控制台…」 |
| Linux | 下载 x86_64 或 ARM64 的 `unknown-linux-musl.tar.gz`，解压后运行 `./codex-mixin panel`；也可安装对应 `.deb` | 在浏览器打开终端打印的访问地址 |

Linux 压缩包内含可直接运行的二进制；进入解压后的目录执行即可。Web 面板运行期间请保持进程开启。系统要求、文件名和远程访问方法见 [安装指南](https://github.com/Edward-lyz/codex-mixin/wiki/Installation)。

### 2. 配置模型并接入 Agent

1. 在「模型与服务」添加供应商，填写 API Key 或完成 OAuth 登录。
2. 刷新模型列表，选择要使用的模型并保存，确认网关已启动。
3. 打开 Agent 页面，找到已安装的客户端并接入；Codex 通常选择「官方账号模式」，保留官方 GPT 与 ChatGPT 登录。
4. 重启对应客户端或打开新会话，在模型选择器中选择自定义模型。

### 3. 查看状态与用量

菜单栏或托盘可快速查看网关状态和供应商额度；控制台的「用量明细」展示每天与每次回复的 Token、缓存和速度。需要恢复客户端原配置时，在 Agent 页面断开接入。

Web 面板默认监听所有网卡的 8000–9000 随机端口，访问需要本次启动生成的 Token。远程访问时，使用开发机可访问的地址和面板实际端口；网关与面板的访问边界见 [数据与安全](https://github.com/Edward-lyz/codex-mixin/wiki/Data-and-Security)。

---

## 界面

以下为 macOS App 实际界面。

<p align="center">
  <a href="docs/assets/APP-Console-Models-v074.png"><img src="docs/assets/APP-Console-Models-v074.png" alt="macOS 控制台的模型与服务页面"></a><br>
  <sub><b>模型与服务</b> · 供应商、模型选择、能力与上下文设置</sub>
</p>

<table>
  <tr>
    <td width="50%" align="center">
      <a href="docs/assets/APP-Console-Agents-v074.png"><img src="docs/assets/APP-Console-Agents-v074.png" alt="macOS 控制台的 Agent 接入页面"></a><br>
      <sub><b>Agent 接入</b> · 安装版本、接入状态与恢复入口</sub>
    </td>
    <td width="50%" align="center">
      <a href="docs/assets/APP-Console-Usage-v074.png"><img src="docs/assets/APP-Console-Usage-v074.png" alt="macOS 控制台的用量明细页面"></a><br>
      <sub><b>用量明细</b> · 每日统计、逐次回复、缓存与速度</sub>
    </td>
  </tr>
</table>

Windows 原生 WinUI 3 界面：托盘查看额度，控制台管理模型与服务、Agent 接入、用量明细和上下文。

<p align="center">
  <a href="docs/assets/Windows-Tray.png"><img src="docs/assets/Windows-Tray.png" width="360" alt="Windows 浅色模式托盘，显示网关状态与额度圆环"></a>
</p>

<p align="center">
  <a href="docs/assets/Windows-Console-Models.png"><img src="docs/assets/Windows-Console-Models.png" alt="Windows 控制台的模型与服务页面"></a>
</p>

界面说明见 Wiki 的 [产品展示](https://github.com/Edward-lyz/codex-mixin/wiki/Product-Tour)。截图中的模型和统计值仅反映采集时状态。

---

## 核心功能

**【NEW】上下文引擎。** 长任务边推进、边压缩：使用 Billion Context 原版算法，在 mixin 内部集成，可以实现在推理的同时，分段压缩替换历史，在保持缓存命中率 90% 以上的同时，让 1M 窗口的上下文能塞进 1B 的内容。

<img src="docs/assets/context-engine-comparison.svg" alt="上下文压缩机制对比" style="zoom:50%;" />

**与官方 Codex 共存。** 自定义模型以独立名称加入 Codex 模型目录，与官方模型并列显示。官方目录从 OpenAI 同步，接入和断开都不改动 Codex 的会话数据库与账号状态。

**协议转换。** 网关在 OpenAI Responses、Chat Completions、Anthropic Messages 和 Gemini 之间转换请求与流式响应，保留思考内容、工具调用和多轮上下文。

**模型能力探测。** 对已添加的模型发送真实请求，标注 Reasoning、Function Calling、Tool Search 与 Web Search 能力，结果缓存后供目录和 Fusion 使用。

**用量与额度。** 查看每天和每次回复的 Token、缓存命中率、思考消耗、返回模型、首字延迟与输出速度，本地保留最近 5000 条回复。供应商额度支持圆环或条形显示，已用超过 90% 时标红。

**模型融合（Fusion）。** 一个模型标识可以调度主模型和辅助模型，由 Judge 模型评审结果；也可以按时间段把同一标识路由到不同供应商。

**配置备份。** 在高级菜单导出或导入供应商、模型与 Fusion 配置，用于恢复或跨机器迁移。备份包含 API Key，Base64 只是编码，请按凭据妥善保管。

**官方 GPT 的 ECH 连接（实验性）。** 在高级菜单启用前会先测试 DNS 与 TLS，再通过 DoH 与 ECH 访问 OpenAI 官方端点。详见 [ECH GPT](docs/ech-gpt.md)。

**客户端鉴权（可选）。** 默认构建不校验入站 Key，便于本机客户端直接使用。以 `--features client-auth` 构建时，网关要求客户端携带密钥。

---

## 供应商

在界面中添加供应商时可选择以下预设，其余兼容端点选择「自定义」。

| 预设 | 供应商 | 上游协议 | 鉴权 |
| --- | --- | --- | --- |
| `openai` | OpenAI 官方 | OpenAI Responses | ChatGPT OAuth |
| `baidu-oneapi` | Baidu OneAPI | Anthropic Messages；GPT 系列走 OpenAI Responses | API Key |
| `openrouter` | OpenRouter | OpenAI Chat Completions | API Key |
| `deepseek` | DeepSeek | OpenAI Responses | API Key |
| `agy` | Google Antigravity | Gemini | Google OAuth |
| `opencode-go` | OpenCode Go | OpenAI Responses | API Key，自动附加会话亲和 Header |
| `aws-bedrock` | Amazon Bedrock (Mantle) | Anthropic Messages | Bedrock API Key 或 AWS 访问密钥 |
| `custom` | 任意兼容端点 | OpenAI Responses（默认） | 自定义 Header 与 Key |

添加后可在界面中测试连接、刷新模型列表，并从模型操作菜单重新探测能力；探测会发送真实请求，可能消耗额度。

---

## Agent 接入

50 款 Agent 均提供接入或原生模型管理。47 款支持网关配置或原生应用接入，断开时恢复托管字段并保留无关配置。Cursor CLI、Copilot CLI、Devin 使用厂商账号管理厂商原生模型，不经过自定义模型网关。

### 已支持 Agent（50 款）

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
    <td width="20%" align="center">
      <img src="macos/assets/agents/gemini.svg" width="36" height="36" alt="Gemini CLI logo"><br>
      <sub>Gemini CLI</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/agy.svg" width="36" height="36" alt="agy logo"><br>
      <sub>agy</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="docs/assets/agents/cursor-light.svg#gh-light-mode-only" width="36" height="36" alt="Cursor CLI logo"><img src="docs/assets/agents/cursor-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Cursor CLI logo"><br>
      <sub>Cursor CLI · 原生模型</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/cursor-local-light.svg#gh-light-mode-only" width="36" height="36" alt="Cursor logo"><img src="docs/assets/agents/cursor-local-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Cursor logo"><br>
      <sub>Cursor</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/copilot-light.svg#gh-light-mode-only" width="36" height="36" alt="Copilot CLI logo"><img src="docs/assets/agents/copilot-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Copilot CLI logo"><br>
      <sub>Copilot CLI · 原生模型</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/devin-light.svg#gh-light-mode-only" width="36" height="36" alt="Devin logo"><img src="docs/assets/agents/devin-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Devin logo"><br>
      <sub>Devin · 原生模型</sub>
    </td>
    <td width="20%" align="center">
      <img src="docs/assets/agents/zed-light.svg#gh-light-mode-only" width="36" height="36" alt="Zed logo"><img src="docs/assets/agents/zed-dark.svg#gh-dark-mode-only" width="36" height="36" alt="Zed logo"><br>
      <sub>Zed</sub>
    </td>
  </tr>
  <tr>
    <td width="20%" align="center">
      <img src="macos/assets/agents/aside.svg" width="36" height="36" alt="Aside logo"><br>
      <sub>Aside</sub>
    </td>
    <td width="20%" align="center">
      <img src="macos/assets/agents/muse.svg" width="36" height="36" alt="Muse logo"><br>
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
</table>

**原生应用前提。** Alma、Hanako 需要保持运行；Zed 使用系统凭据存储。Aside 离线时先暂存配置，待原生运行时确认后才显示已接入。Cindy 需要在应用内确认导入和手动删除提供商；等待删除时，断开操作已撤销网关访问。

**必须从终端启动。** 接入 agy、Cursor、Muse、Command Code，以及开启客户端鉴权的 fx / Empryo / Ante 后，请重新打开终端，再照常输入 `agy`、`cursor`、`muse`、`command-code` 等命令。Mixin 在 zsh、bash、fish 或 PowerShell 的用户启动配置中加入可恢复的入口，自动注入托管环境；无需手动使用 `agent run`。Cursor 必须先完全退出已有进程，再从终端执行 `cursor`，Dock、开始菜单和桌面图标启动不受托管；Windows 的 cmd 也不加载这些入口。密钥只保存在私有托管记录和子进程环境中，启动配置只保存 Mixin 路径与配置文件路径。断开后重新打开终端，原生命令恢复。也可显式使用 `codex-mixin agent run <id> -- <参数>` 或 `codex-mixin agent launch <id>`。

**Gemini 模型选择。** 接入后重启 Gemini，在 `/model` 的手动选择中查看标为 `Codex Mixin:` 的自定义模型。旧版接入请先断开再重新接入，以更新原生模型目录。

**厂商原生模型。** 例如 `codex-mixin agent model cursor --model <厂商模型ID>`；将 `cursor` 换成 `copilot` 或 `devin` 可管理对应原生设置，Copilot 另支持 `--effort high`。接入时沿用已有的厂商原生模型；尚未选择时，先用上述命令指定模型。这些命令不创建网关凭据，也不要求先选择网关模型。各应用前提、恢复方式和 Gemini Token 估算语义见 [Agent 接入契约](docs/agent-integration-contracts.md)。



仓库内文档：

- [架构与模块边界](docs/architecture.md)
- [Agent 接入契约](docs/agent-integration-contracts.md)
- [Google Antigravity](docs/antigravity.md)
- [官方 GPT 的 ECH 直连](docs/ech-gpt.md)
- [浏览器跨域白名单](docs/cors.md)
- [长上下文引擎](docs/context-engine.md)

使用教程见 [GitHub Wiki](https://github.com/Edward-lyz/codex-mixin/wiki)：

- [Product Tour](https://github.com/Edward-lyz/codex-mixin/wiki/Product-Tour)
- [安装](https://github.com/Edward-lyz/codex-mixin/wiki/Installation) 与 [快速上手](https://github.com/Edward-lyz/codex-mixin/wiki/Quick-Start)
- [用量与额度](https://github.com/Edward-lyz/codex-mixin/wiki/Usage-and-Quota)
- [配置备份与恢复](https://github.com/Edward-lyz/codex-mixin/wiki/Configuration-Backup-and-Restore)
- [供应商与模型](https://github.com/Edward-lyz/codex-mixin/wiki/Providers-and-Models)
- [客户端接入](https://github.com/Edward-lyz/codex-mixin/wiki/Client-Integrations)
- [CLI 参考](https://github.com/Edward-lyz/codex-mixin/wiki/CLI-Reference)
- [故障排查](https://github.com/Edward-lyz/codex-mixin/wiki/Troubleshooting) 与 [FAQ](https://github.com/Edward-lyz/codex-mixin/wiki/FAQ)

版本变更见 [RELEASE_NOTES.md](RELEASE_NOTES.md)。

---

## 许可证

本项目以 [LICENSE](LICENSE) 发布，第三方声明见 [NOTICE](NOTICE)。
