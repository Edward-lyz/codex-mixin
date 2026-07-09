# Codex Mixin 产品介绍

![Codex Mixin](assets/app-icon.png)

Codex Mixin 是一个面向 Codex 用户的 macOS 菜单栏工具。它把自定义模型供应商接入 Codex，同时保留 Codex 官方 ChatGPT/OpenAI 账号带来的远程控制、官方 GPT 模型和原生体验。

![设置供应商与密钥](assets/settings.png)

## 背景痛点

很多团队已经有自己的模型入口，例如内部 OneAPI、OpenRouter、DeepSeek 或其他 Anthropic/OpenAI 兼容网关。但 Codex 的真实使用场景不只是发一次 API 请求，用户还希望保留这些能力：

- 继续使用 ChatGPT 账号登录后的官方 Codex 能力。
- 官方 GPT 模型和自定义模型能在同一个模型列表里选择。
- 新会话能用自定义模型，旧会话不能因为默认 provider 被改掉而消失。
- 模型 catalog 必须符合 Codex 期望，不能缺 context window、instructions template 等字段。
- 网关要能长期运行，不能依赖一个没关的终端窗口。
- 普通用户不想理解 `/v1/messages`、`/v1/chat/completions`、`/anthropic` 这些路径差异。
- API 额度要能看，但不能把一长串 JSON 直接塞进菜单。

直接改 `~/.codex/config.toml` 可以临时跑通，但风险很高：容易覆盖官方 provider、顶掉官方 GPT、破坏历史会话索引，也很难向更多用户推广。

## Codex Mixin 的解法

Codex Mixin 把问题拆成三个层面处理。

第一层是本地网关。Codex 仍然连接本机 `http://127.0.0.1:8787/v1`，网关负责把 Codex Responses 请求转成上游需要的 Anthropic Messages 或 OpenAI Chat Completions 请求，再把流式结果转回 Codex 能理解的格式。

第二层是 Codex 配置托管。安装时不会直接把用户原始配置改没，而是先备份 `~/.codex/config.toml`，再写入托管配置和独立模型目录 `~/.codex/model-catalogs/mixin-models.json`。卸载时恢复安装前配置，删除托管目录。

第三层是 macOS 产品外壳。用户通过菜单栏完成启动、暂停、重启、设置供应商、安装到 Codex、恢复官方配置、查看额度、打开日志等动作，不需要记一组 CLI 命令。

## 对用户的直接价值

- 保留官方路径：官方 GPT 模型继续走 Codex 官方认证和远程控制路径。
- 接入自定义模型：自定义模型出现在 Codex 模型选择器里，和官方模型一起使用。
- 避免模型冲突：自定义上游返回的 GPT 系模型会自动加 `-custom` 后缀，不顶掉官方 GPT。
- 保护历史会话：安装时保留 Codex 当前默认 provider，只把地址改到本地网关，减少会话列表突然变空的问题。
- 安装可回滚：每次安装前备份，菜单里可以一键恢复官方配置。
- 降低配置负担：内置 Baidu OneAPI、OpenRouter、DeepSeek 供应商预设。
- 额度显示可读：菜单栏显示已用额度和进度条，而不是展示原始 JSON。
- 服务常驻：launchd 托管本地网关，关闭终端后服务仍可用。

## 支持的供应商

当前内置供应商：

| 供应商 | 适用场景 | 协议 |
| --- | --- | --- |
| Baidu OneAPI | 厂内 OneAPI / Comate 模型入口 | Anthropic Messages |
| OpenRouter | 聚合模型市场 | OpenAI Chat Completions |
| DeepSeek | DeepSeek 官方 API | OpenAI Chat Completions |
| Custom | 任意兼容网关 | Anthropic Messages 默认，可通过配置切换 |

供应商预设会自动补齐常见 base URL、models path、generation path 和 quota path。设置窗口里只填写根地址，避免用户纠结到底要不要写 `/v1/chat/completions`。

## 为什么不是简单脚本

这个需求的难点不在一次 HTTP 转发，而在 Codex 的整体使用路径：

- Codex 会校验模型 catalog 的结构。
- 官方 GPT 与自定义 GPT 需要同名避让。
- Codex App 和 CLI 对配置变更的生效时机不同。
- 历史会话和 provider 绑定，随意改默认 provider 会造成会话看起来丢失。
- 上游 streaming、tool call、thinking、web search、WebSocket 都需要保持协议语义。
- 用户希望它像一个产品，而不是一个需要守着终端的 demo。

Codex Mixin 因此把 CLI、网关、模型 catalog、配置备份恢复、launchd 和菜单栏 UI 一起做成一个完整工具。

## 产品边界

Codex Mixin 不绕过 Codex 官方账号体系。推荐安装模式下，官方 GPT 仍走官方 Codex/OpenAI 路径；本地网关只负责自定义模型和协议转换。

Codex Mixin 也不伪造上游能力。上游不支持的能力会明确失败或在 catalog 中按可验证规则标注，不做静默 fallback。

安装或卸载 Codex 配置后，Codex App 需要重启；Codex CLI 需要开启新会话。这是 Codex 读取配置的生命周期决定的，不应该在工具里假装热更新已经完成。

## 推荐使用方式

1. 打开 Codex Mixin 菜单栏 app。
2. 在 `设置供应商与密钥...` 里选择供应商并保存 API Key。
3. 点击 `启动本地网关`。
4. 点击 `安装到 Codex...`。
5. 重启 Codex App，或重新打开 Codex CLI 会话。
6. 在 Codex 模型选择器里选择官方 GPT 或自定义模型。

需要恢复时，点击 `从 Codex 恢复...`，再重启 Codex。

## 适合谁

- 已经在 Codex 中重度使用官方 GPT 模型，但希望补充内部或第三方模型的用户。
- 需要让团队成员低成本接入统一模型入口的工程团队。
- 希望保留 Codex 官方认证和远程控制，同时用自定义模型做实验的开发者。
- 维护 Anthropic/OpenAI 兼容 API 网关，希望直接接入 Codex 的平台团队。

## 当前状态

Codex Mixin 已具备本地网关、菜单栏 app、launchd 常驻、供应商预设、模型 catalog 生成、上下文 metadata 补齐、Codex 配置安装/恢复、额度显示和截图中的设置界面。

后续更适合继续补的是打包签名、自动更新、更多供应商预设，以及针对团队分发的配置模板。
