# Codex Mixin

<p align="center">
  <img src="docs/assets/app-icon.png" width="120" alt="Codex Mixin icon">
</p>

<p align="center">
  <a href="https://github.com/Edward-lyz/codex-mixin/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/Edward-lyz/codex-mixin/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/Edward-lyz/codex-mixin/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/Edward-lyz/codex-mixin?sort=semver"></a>
  <a href="https://github.com/Edward-lyz/codex-mixin/releases"><img alt="macOS and Linux" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-blue"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-PolyForm%20Noncommercial%201.0.0-lightgrey"></a>
  <img alt="Rust" src="https://img.shields.io/badge/Rust-local%20gateway-orange">
</p>

<p align="center">
  <b>Bring custom model providers into official Codex without giving up ChatGPT account features.</b>
</p>

<p align="center">
  <a href="#中文">中文</a> ·
  <a href="#english">English</a> ·
  <a href="https://github.com/Edward-lyz/codex-mixin/releases/latest">Download</a> ·
  <a href="https://github.com/Edward-lyz/codex-mixin/issues">Issues</a>
</p>

![Codex model picker with custom models](docs/assets/codex-model-picker.png)

## 中文

Codex Mixin 是一个 Rust 本地网关、CLI 和 macOS 菜单栏 App。它把 OpenRouter、DeepSeek、Baidu OneAPI 或其他 OpenAI Chat Completions / Anthropic Messages 兼容模型接入官方 Codex，同时保留官方 ChatGPT/OpenAI 账号路径、官方 GPT 模型、远程控制和 Codex 原生体验。

它不是 Codex 的二次发行版，也不重新打包官方 Codex App。Codex 仍然是主入口，Codex Mixin 只负责模型接入、协议转换、模型目录生成、配置托管、服务常驻和额度展示。

### 目录

- [为什么需要它](#为什么需要它)
- [功能特性](#功能特性)
- [快速安装](#快速安装)
- [快速使用](#快速使用)
- [供应商预设](#供应商预设)
- [安装到 Codex 的行为](#安装到-codex-的行为)
- [菜单栏 App](#菜单栏-app)
- [CLI](#cli)
- [模型目录和 metadata](#模型目录和-metadata)
- [Thinking 与 Web Search](#thinking-与-web-search)
- [数据位置](#数据位置)
- [开发与发布](#开发与发布)
- [许可证](#许可证)
- [常见问题](#常见问题)

### 为什么需要它

很多团队和个人已经有自己的模型入口，例如内部 OneAPI、OpenRouter、DeepSeek 或自建兼容网关。但 Codex 的真实使用场景不只是发一次 API 请求，用户还希望保留这些能力：

- 继续使用 ChatGPT 账号登录后的官方 Codex 能力。
- 官方 GPT 模型和自定义模型能在同一个模型选择器里出现。
- 新会话可以用自定义模型，旧会话不会因为 provider 被改掉而看起来消失。
- Codex model catalog 字段完整，不缺 context window、instructions template 等必需字段。
- 本地网关能长期运行，不依赖一个不能关闭的终端窗口。
- 普通用户不需要理解 `/v1/messages`、`/v1/chat/completions`、`/anthropic` 等路径差异。
- API 额度能在菜单栏里用可读方式展示，而不是显示一整段原始 JSON。

Codex Mixin 的解法是：Codex 连到本机 `http://127.0.0.1:8787/v1`，本地网关再按 provider 把请求转成上游需要的协议，并把流式响应转回 Codex 能理解的 Responses 形态。

### 功能特性

- 保留官方路径：官方 GPT 模型继续走 Codex 官方认证、官方后端和远程控制路径。
- 接入自定义模型：自定义模型进入 Codex 模型选择器，和官方模型一起使用。
- 避免模型冲突：自定义上游返回的 `gpt-*` 会安装为 `gpt-...-custom`，不顶掉官方 GPT。
- 保护历史会话：推荐安装模式保留当前 Codex provider，只把该 provider 的 `base_url` 指向本地网关。
- 可回滚配置：安装前备份 `~/.codex/config.toml`，卸载时恢复备份并删除托管模型目录。
- 供应商预设：内置 `custom`、`baidu-oneapi`、`openrouter`、`deepseek`。
- 协议转换：支持 Anthropic Messages 和 OpenAI Chat Completions 上游。
- 模型 metadata 补齐：结合 LiteLLM metadata 和内置正则规则补齐上下文窗口、能力和 instruction 字段。
- 菜单栏产品化：启动、暂停、重启、配置密钥、安装到 Codex、恢复、查看额度和日志都在菜单栏完成。
- 常驻服务：macOS 使用 launchd 托管，关闭终端或退出菜单栏 App 后网关仍可继续运行。
- 自动更新：菜单栏 App 可检查 GitHub Release，并下载当前架构对应的 DMG。

### 快速安装

从 [GitHub Releases](https://github.com/Edward-lyz/codex-mixin/releases/latest) 下载当前 Mac 架构对应的 DMG：

| Mac 架构 | 下载文件 |
| --- | --- |
| Apple Silicon | `codex-mixin-0.2.2-aarch64-apple-darwin.dmg` |
| Intel | `codex-mixin-0.2.2-x86_64-apple-darwin.dmg` |

打开 DMG，把 `Codex Mixin.app` 拖到 `Applications`，然后启动菜单栏 App。

当前发布包尚未签名和 notarize。如果 macOS 拦截，执行下面命令后再打开：

```bash
xattr -dr com.apple.quarantine codex-mixin-0.2.2-aarch64-apple-darwin.dmg
xattr -dr com.apple.quarantine "/Applications/Codex Mixin.app"
```

打开后按菜单栏提示完成配置。远端开发机或 Linux 用户可以从 Release 页面下载 CLI 包自行使用。

<details>
<summary>CLI 下载文件名</summary>

- macOS Apple Silicon: `codex-mixin-cli-0.2.2-aarch64-apple-darwin.tar.gz`
- macOS Intel: `codex-mixin-cli-0.2.2-x86_64-apple-darwin.tar.gz`
- Linux x86_64: `codex-mixin-cli-0.2.2-x86_64-unknown-linux-gnu.tar.gz` 或 `codex-mixin-0.2.2-x86_64-unknown-linux-gnu.deb`
- Linux ARM64: `codex-mixin-cli-0.2.2-aarch64-unknown-linux-gnu.tar.gz` 或 `codex-mixin-0.2.2-aarch64-unknown-linux-gnu.deb`

</details>

### 快速使用

#### 本地 Codex App 用户

1. 打开 `Codex Mixin.app`。
2. 点击菜单栏图标，选择 `设置供应商与密钥...`。
3. 选择 provider，填入 API Key。上游地址只填根地址，不要填 `/v1/messages` 或 `/v1/chat/completions`。
4. 点击 `启动本地网关`。
5. 点击 `安装到 Codex...`。
6. 重启 Codex App。
7. 在 Codex 模型选择器里选择官方 GPT 或自定义模型。

![Menu bar status](docs/assets/menu-status.png)

#### 远端 Codex CLI 用户

```bash
codex-mixin login --provider openrouter --key sk-or-v1-...
codex-mixin doctor
codex-mixin install-codex --codex-oauth-proxy
codex-mixin start --daemon
```

然后重新打开 Codex CLI 会话，在模型选择器里选择接入后的模型。

常用检查命令：

```bash
codex-mixin status
codex-mixin models --json
codex-mixin quota --json
codex-mixin logs -n 200
```

### 供应商预设

| Provider | 上游协议 | 上游根地址 | 生成接口 | 模型接口 | 额度接口 |
| --- | --- | --- | --- | --- | --- |
| `custom` | Anthropic Messages 默认 | 用户填写 | `/v1/messages` | `/v1/models` | 无默认值 |
| `baidu-oneapi` | Anthropic Messages | `https://oneapi-comate.baidu-int.com` | `/v1/messages` | `/v1/models` | `/openapi/v3/user/quota` |
| `openrouter` | OpenAI Chat Completions | `https://openrouter.ai/api` | `/v1/chat/completions` | `/v1/models` | `/v1/credits` |
| `deepseek` | OpenAI Chat Completions | `https://api.deepseek.com` | `/chat/completions` | `/models` | 无默认值 |

设置窗口里的上游地址只填根地址。路径由 provider preset 补齐。

示例：

- OpenRouter 填 `https://openrouter.ai/api`，不要填 `/v1/chat/completions`。
- DeepSeek 填 `https://api.deepseek.com`，不要填 `/chat/completions`。
- Anthropic Messages 兼容网关通常填网关根地址，`custom` 会默认使用 `/v1/messages` 和 `/v1/models`。

![Provider select](docs/assets/provider-select.png)

![Provider config](docs/assets/provider-config.png)

### 安装到 Codex 的行为

推荐命令：

```bash
codex-mixin install-codex --codex-oauth-proxy
```

安装会做这些事：

1. 读取上游 models 接口，生成 Codex 可用的模型目录。
2. 写入独立模型目录文件 `~/.codex/model-catalogs/mixin-models.json`。
3. 备份当前 `~/.codex/config.toml`。
4. 写入托管配置，并保留当前默认 provider。
5. 只把当前 provider 的 `base_url` 指向本地网关。
6. 标记该 provider 仍然使用 Codex 官方 OAuth 能力。

关键配置形态：

```toml
model_catalog_json = "/Users/you/.codex/model-catalogs/mixin-models.json"

[model_providers.openai]
name = "OpenAI"
base_url = "http://127.0.0.1:8787/v1"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = true
```

如果原配置里有 `model_provider`，Codex Mixin 会更新那个 provider。如果没有显式 `model_provider`，按 Codex 默认的 `openai` provider 处理。

默认不会改顶层 `model`。如果确实要顺手设置默认模型，可以显式传入：

```bash
codex-mixin install-codex --codex-oauth-proxy --model deepseek-chat --set-default
```

卸载并恢复安装前配置：

```bash
codex-mixin uninstall-codex
```

安装或卸载后需要重启 Codex App。Codex CLI 需要开启新会话。

### 菜单栏 App

菜单栏 App 提供这些动作：

- `启动本地网关`：写入并加载用户级 launchd agent。
- `暂停本地网关`：卸载 launchd agent，并停止当前 daemon。
- `重启本地网关`：重新写入 launchd agent 并启动服务。
- `登录时启动并开启服务`：控制用户登录后是否自动启动网关。
- `刷新状态与额度`：刷新服务状态和额度进度条。
- `设置供应商与密钥...`：选择 provider、填写 API Key、上游根地址、本地保护密钥和额度接口。
- `安装到 Codex...`：生成模型目录并写入托管 Codex 配置。
- `从 Codex 恢复...`：恢复安装前备份并删除托管模型目录。
- `检查更新...`：查询 GitHub 最新 release，下载并打开当前架构对应的 DMG。
- `自动检查更新`：每天最多检查一次，发现新版本时提示下载。
- `复制本地接口地址`：复制 `http://127.0.0.1:8787/v1`。
- `打开运行日志`：打开 `~/.codex-mixin/gateway.log`。
- `打开配置目录`：打开 `~/.codex-mixin`。

服务由 launchd 托管。关闭终端或退出菜单栏 App 后，本地网关仍可以继续运行。需要临时停服务时使用菜单里的暂停动作；需要取消开机自启时关闭 `登录时启动并开启服务`。

### CLI

```bash
codex-mixin login
codex-mixin logout
codex-mixin doctor
codex-mixin status
codex-mixin models --json
codex-mixin quota --json
codex-mixin config --json
codex-mixin start --daemon
codex-mixin stop
codex-mixin restart
codex-mixin logs -n 200
codex-mixin catalog
codex-mixin refresh-metadata
codex-mixin install-codex --codex-oauth-proxy
codex-mixin uninstall-codex
codex-mixin migrate-history
```

`serve` 仍保留为前台 `start` 的兼容别名。新文档和菜单栏 App 统一使用 `start`。

### 模型目录和 metadata

很多上游 `/models` 只返回模型 ID。Codex Mixin 生成 catalog 时会按以下顺序补齐上下文窗口和能力字段：

1. `CODEX_GATEWAY_MODEL_METADATA` 指向的本地 metadata 文件。
2. `~/.codex-mixin/model_metadata_litellm.json`，由 `refresh-metadata` 或安装时自动拉取 LiteLLM metadata 生成。
3. 内置模型族正则规则，例如 Claude、DeepSeek、GPT、Kimi、GLM、MiniMax 等常见命名。

生成的 catalog 会包含 `context_window`、`max_context_window`、`input_modalities`、`base_instructions` 和 `model_messages.instructions_template`，避免 Codex 解析模型目录时报缺字段。

### Thinking 与 Web Search

Anthropic 风格上游支持 Codex reasoning effort 到 thinking 的映射：

| Codex effort | Anthropic thinking |
| --- | --- |
| `minimal` / `low` | `low` |
| `medium` | `medium` |
| `high` | `high` |
| `xhigh` / `exhigh` / `max` | `max` |

未知 effort 会返回 400，而不是静默降级到错误档位。

Web search 转发默认关闭，需要显式开启：

```bash
CODEX_GATEWAY_ENABLE_WEB_SEARCH_TOOL=true
CODEX_GATEWAY_WEB_SEARCH_TOOL_TYPE=web_search_20250305
CODEX_GATEWAY_WEB_SEARCH_MAX_USES=3
```

### 数据位置

| 内容 | 路径 |
| --- | --- |
| Codex Mixin 配置 | `~/.codex-mixin/config.json` |
| 本地网关日志 | `~/.codex-mixin/gateway.log` |
| LiteLLM metadata 缓存 | `~/.codex-mixin/model_metadata_litellm.json` |
| Codex 配置 | `~/.codex/config.toml` |
| Codex 配置备份 | `~/.codex/config.toml.codex-mixin.backup` |
| Codex 模型目录 | `~/.codex/model-catalogs/mixin-models.json` |

做 Codex 配置实验时不要直接碰真实配置，可以使用隔离目录：

```bash
CODEX_HOME=/tmp/codex-mixin-home codex-mixin install-codex --codex-oauth-proxy
```

### 开发与发布

本地检查：

```bash
cargo fmt --all -- --check
cargo test --locked
./macos/build_app.sh
```

Release workflow 在推送 `v*` tag 或手动运行时生成：

| 平台 | 架构 | CLI 包 | 安装包 |
| --- | --- | --- | --- |
| Linux | `x86_64` | `.tar.gz` | `.deb` |
| Linux | `aarch64` | `.tar.gz` | `.deb` |
| macOS | `x86_64` | `.tar.gz` | `.dmg` |
| macOS | `aarch64` | `.tar.gz` | `.dmg` |

macOS DMG 内包含 `Codex Mixin.app`、`bin/codex-mixin`、`README.md` 和 `Applications` 快捷入口，并带有 Finder 窗口布局和背景图。Linux `.deb` 会把 CLI 安装到 `/usr/local/bin/codex-mixin`。

### 许可证

Codex Mixin 使用 [PolyForm Noncommercial License 1.0.0](LICENSE)。

这意味着你可以为非商业目的使用、复制、修改和分发源码及其修改版本；不能把它用于商业目的。这个许可证是 source-available / non-commercial license，不是 OSI open source license。

分发副本或修改版本时，请同时保留 `LICENSE` 和 `NOTICE`。

### 常见问题

#### 安装后为什么要重启 Codex App？

Codex App 读取配置有自己的生命周期。安装或恢复 Codex 配置后，需要重启 Codex App 才能看到最新模型目录。Codex CLI 需要重新开启新会话。

#### 官方 GPT 会走本地网关吗？

推荐的 `--codex-oauth-proxy` 模式会保留官方 OAuth provider 能力。官方 GPT 模型继续走官方 Codex/OpenAI 路径；自定义模型通过本地网关转发到你的 provider。

#### 为什么不直接做一个新的 Codex App？

官方 Codex App 的交互、插件、权限模型和工具运行时更新很快。二次开发 App 容易变成长期追版本。Codex Mixin 选择增强官方 App，而不是替代官方 App。

#### 菜单栏额度显示支持哪些 provider？

`baidu-oneapi` 和 `openrouter` 有默认额度接口。其他 provider 可以在设置窗口里填自定义额度接口。Codex Mixin 会从常见 JSON 字段中提取 used / limit / remaining 并显示进度条；无法识别时会显示明确的查询结果或错误。

#### API Key 存在哪里？

默认保存在 `~/.codex-mixin/config.json`。这是本机用户目录下的配置文件。不要把它提交到 Git。

#### 反馈问题时应该带什么？

请在 [GitHub Issues](https://github.com/Edward-lyz/codex-mixin/issues) 提供：

- Codex Mixin 版本。
- Codex App / Codex CLI 版本。
- 使用菜单栏 App 还是 CLI。
- provider 类型。
- 问题截图。
- `codex-mixin doctor` 输出。
- `codex-mixin logs -n 200` 输出。

## English

Codex Mixin is a local Rust gateway, CLI, and macOS menu bar app for connecting custom model providers to official Codex while keeping ChatGPT/OpenAI account features, official GPT models, remote control, and the native Codex experience.

It is not a fork or repackaged Codex Desktop. Codex remains the main UI. Codex Mixin only handles provider setup, protocol translation, model catalog generation, managed config updates, daemon lifecycle, quota display, and rollback.

### Why

Many users already have model access through internal OneAPI gateways, OpenRouter, DeepSeek, or self-hosted OpenAI / Anthropic compatible APIs. A simple `base_url` patch is not enough for Codex because real usage needs:

- Official ChatGPT account features to keep working.
- Official GPT models and custom models in the same model picker.
- Existing sessions to stay visible instead of disappearing after a provider switch.
- A valid Codex model catalog with context window and instruction fields.
- A local service that survives terminal exits.
- Provider presets so users do not need to know every endpoint path.
- Human-readable quota status instead of raw JSON in the menu bar.

Codex Mixin exposes a local Responses-compatible endpoint at `http://127.0.0.1:8787/v1`, translates requests to Anthropic Messages or OpenAI Chat Completions upstreams, then translates streaming responses back for Codex.

### Features

- Keeps official Codex/OpenAI account path for official GPT models.
- Adds custom upstream models to the Codex model picker.
- Avoids GPT name collisions by installing upstream `gpt-*` models as `gpt-...-custom`.
- Preserves the current Codex provider and only changes its `base_url` in the recommended install mode.
- Backs up `~/.codex/config.toml` before managed changes and restores it on uninstall.
- Includes provider presets for `custom`, `baidu-oneapi`, `openrouter`, and `deepseek`.
- Supports Anthropic Messages and OpenAI Chat Completions upstreams.
- Completes model metadata using LiteLLM metadata plus built-in model-family rules.
- Provides a macOS menu bar control surface for service lifecycle, provider setup, Codex install, rollback, quota, logs, and updates.
- Runs as a launchd-managed service on macOS.

### Install

Download the DMG for your Mac from [GitHub Releases](https://github.com/Edward-lyz/codex-mixin/releases/latest):

| Mac | File |
| --- | --- |
| Apple Silicon | `codex-mixin-0.2.2-aarch64-apple-darwin.dmg` |
| Intel | `codex-mixin-0.2.2-x86_64-apple-darwin.dmg` |

Open the DMG, drag `Codex Mixin.app` to `Applications`, then launch it.

The current builds are not signed or notarized. If macOS blocks the app, run:

```bash
xattr -dr com.apple.quarantine codex-mixin-0.2.2-aarch64-apple-darwin.dmg
xattr -dr com.apple.quarantine "/Applications/Codex Mixin.app"
```

After launch, follow the menu bar actions to configure a provider and install it into Codex. Remote Linux or Codex CLI users can download the CLI archives from the same Release page.

<details>
<summary>CLI asset names</summary>

- macOS Apple Silicon: `codex-mixin-cli-0.2.2-aarch64-apple-darwin.tar.gz`
- macOS Intel: `codex-mixin-cli-0.2.2-x86_64-apple-darwin.tar.gz`
- Linux x86_64: `codex-mixin-cli-0.2.2-x86_64-unknown-linux-gnu.tar.gz` or `codex-mixin-0.2.2-x86_64-unknown-linux-gnu.deb`
- Linux ARM64: `codex-mixin-cli-0.2.2-aarch64-unknown-linux-gnu.tar.gz` or `codex-mixin-0.2.2-aarch64-unknown-linux-gnu.deb`

</details>

### Usage

#### For Codex Desktop on macOS

1. Open `Codex Mixin.app`.
2. Open `Set Provider and Key...` from the menu bar.
3. Choose a provider and enter your API key. Only enter the upstream root URL, not `/v1/messages` or `/v1/chat/completions`.
4. Click `Start Local Gateway`.
5. Click `Install to Codex...`.
6. Restart Codex Desktop.
7. Pick an official GPT model or a custom model in Codex.

#### For Codex CLI

```bash
codex-mixin login --provider openrouter --key sk-or-v1-...
codex-mixin doctor
codex-mixin install-codex --codex-oauth-proxy
codex-mixin start --daemon
```

Then start a new Codex CLI session.

### Provider Presets

| Provider | Upstream protocol | Base URL | Generation path | Models path | Quota path |
| --- | --- | --- | --- | --- | --- |
| `custom` | Anthropic Messages by default | User provided | `/v1/messages` | `/v1/models` | None |
| `baidu-oneapi` | Anthropic Messages | `https://oneapi-comate.baidu-int.com` | `/v1/messages` | `/v1/models` | `/openapi/v3/user/quota` |
| `openrouter` | OpenAI Chat Completions | `https://openrouter.ai/api` | `/v1/chat/completions` | `/v1/models` | `/v1/credits` |
| `deepseek` | OpenAI Chat Completions | `https://api.deepseek.com` | `/chat/completions` | `/models` | None |

Only enter the upstream root URL in the settings window. Codex Mixin adds provider-specific paths.

### Codex Install Behavior

Recommended:

```bash
codex-mixin install-codex --codex-oauth-proxy
```

This command:

1. Fetches upstream models.
2. Generates `~/.codex/model-catalogs/mixin-models.json`.
3. Backs up `~/.codex/config.toml`.
4. Keeps the current Codex provider.
5. Points that provider to `http://127.0.0.1:8787/v1`.
6. Marks the provider as OpenAI-authenticated and websocket-capable.

Example managed shape:

```toml
model_catalog_json = "/Users/you/.codex/model-catalogs/mixin-models.json"

[model_providers.openai]
name = "OpenAI"
base_url = "http://127.0.0.1:8787/v1"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = true
```

Rollback:

```bash
codex-mixin uninstall-codex
```

Restart Codex Desktop after install or uninstall. Start a new session for Codex CLI.

### CLI Reference

```bash
codex-mixin login
codex-mixin logout
codex-mixin doctor
codex-mixin status
codex-mixin models --json
codex-mixin quota --json
codex-mixin config --json
codex-mixin start --daemon
codex-mixin stop
codex-mixin restart
codex-mixin logs -n 200
codex-mixin catalog
codex-mixin refresh-metadata
codex-mixin install-codex --codex-oauth-proxy
codex-mixin uninstall-codex
codex-mixin migrate-history
```

### Files

| Purpose | Path |
| --- | --- |
| Codex Mixin config | `~/.codex-mixin/config.json` |
| Gateway log | `~/.codex-mixin/gateway.log` |
| LiteLLM metadata cache | `~/.codex-mixin/model_metadata_litellm.json` |
| Codex config | `~/.codex/config.toml` |
| Codex config backup | `~/.codex/config.toml.codex-mixin.backup` |
| Codex model catalog | `~/.codex/model-catalogs/mixin-models.json` |

Use an isolated Codex home for experiments:

```bash
CODEX_HOME=/tmp/codex-mixin-home codex-mixin install-codex --codex-oauth-proxy
```

### Development

```bash
git clone https://github.com/Edward-lyz/codex-mixin.git
cd codex-mixin
cargo fmt --all -- --check
cargo test --locked
./macos/build_app.sh
```

Release builds are produced by GitHub Actions for Linux and macOS, x86_64 and aarch64, including CLI archives plus `.deb` or `.dmg` installers.

### License

Codex Mixin is licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE).

You may use, copy, modify, and distribute the source code and modified versions for noncommercial purposes. Commercial use is not permitted. This is a source-available / non-commercial license, not an OSI open source license.

Keep both `LICENSE` and `NOTICE` when distributing copies or modified versions.

### Support

Open an issue at [GitHub Issues](https://github.com/Edward-lyz/codex-mixin/issues) and include:

- Codex Mixin version.
- Codex Desktop / Codex CLI version.
- Whether you use the menu bar app or CLI.
- Provider type.
- Screenshot if applicable.
- `codex-mixin doctor`.
- `codex-mixin logs -n 200`.
