# 行为变更说明（SOLID 重构）

本次重构的外部兼容目标：CLI 命令、正常输出、配置格式与 HTTP/WS 契约保持不变。
以下列出有意调整的 Rust 内部 API 与三处行为修正。

## Rust API 调整

- 新增 `application` 模块与 `application::error::OperationError`
  （`BeforeCommit` / `AfterCommit`）。
- 新增 `upstream` 模块（私有）：`UpstreamAccess` 汇聚 provider/官方发送与
  DUCX、官方认证运行时。`server::AppState` 不再公开 `client` 字段，改为组合
  `upstream`/`gateway`/`catalog` 组件。
- `gateway` 内部由 `UpstreamExecutor` 改为 `GatewayExecutor`（私有），持有
  路由、执行与缓存观测；`fusion::FusionEngine::new` 改为接收
  `&GatewayExecutor`（`pub(crate)`），不再接收 `AppState`。
- 目录生成与缓存从 `server` 迁入 `catalog::CatalogService`（`pub(crate)`）；
  `AppState::fetch_models` / `fetch_official_models_catalog` 仍保留公开入口。
- `server::auth::forward_official_headers` 与转发头清单迁入 `upstream`；
  `stable_oneapi_routing` 迁入 `gateway`。

## 行为修正一：客户端密钥同步

之前每次 CLI 命令分发前都会同步所有已安装客户端的网关密钥，一个损坏的
托管配置会让无关查询（如 `status`、`info`）直接失败。现在：

- 全局分发不再同步密钥。
- 同步只在接入（connect/install）、服务启动、密钥变更与明确修复流程执行；
  服务启动时同步失败仅记录日志，不阻塞网关启动。
- `doctor` 增加 `codex_client_key` 检查，报告托管配置中密钥头缺失或过期，
  并提示重跑 `connect codex`。

## 行为修正二：提交后失败表达

`provider add` / `provider update` 在锁内提交主配置后，还会执行
imagegen skill 同步、模型发现与能力探测。若这些后续步骤失败：

- 返回 `OperationError::AfterCommit`，提示“配置已保存，但 <阶段> 失败”，
  不再暗示 provider 未添加。
- 失败退出码保持不变；已保存的 provider 配置继续存在。
- 后续阶段通过 `provider refresh` 等入口重试，不会重复创建 provider。
- 错误不被吞掉，原始错误链保留在 `AfterCommit.source`。

## 行为修正三：Anthropic 模型级端点

Anthropic Messages 路径原先总是请求 provider 基础端点，而 OpenAI Chat 与
Responses 协议会使用模型级 `api_path`。现在 Anthropic 也走
`api_url_for_model`，在配置了模型级端点时命中该端点；Baidu 路由、AWS SigV4
签名与 DUCX 头部行为不变。

## 未变更

- CLI 命令、参数别名、JSON 输出与退出码。
- 配置格式与迁移语义。
- HTTP/WS 契约、SSE 事件形状、prompt-cache 前缀形状。
- TUI 与 macOS 交互、进度采集。

## 平台无关核心与 CLI 契约

- 网关生命周期（版本检查、LaunchAgent 迁移、就绪等待、自启）从 macOS App
  迁入 CLI，三个平台共用 `service ensure/start/stop/restart --managed` 与
  `service autostart`。Linux 使用 `systemctl --user`，Windows 使用当前用户、
  普通权限的登录计划任务。
- 托管 DUCX 在三个平台使用同一安装流程（`.tar.bz2` 包，Rust 内解压，
  `bin/ducx` 为包内启动器的副本）；无终端时由 CLI 打开终端窗口完成扫码登录，
  macOS App 不再自己生成 Terminal 脚本，也不再假定 DUCX 安装路径。
- `service logs --follow` 在所有平台由 CLI 自己跟随日志，不再依赖 `tail`。
- Codex 会话历史迁移在所有平台使用内置 SQLite，不再依赖 `sqlite3` 命令。
- `doctor --fix` 修复配置文件权限时在 Windows 也会生效（收紧 ACL）。
- 模型列表变更通知在 Linux 桌面会通过 `notify-send` 显示。
- macOS App 的诊断日志改写到 `~/Library/Application Support/Codex Mixin/app.log`，
  不再追加到网关的 `gateway.log`；额度展示缓存与服务商图标缓存同样迁到该目录。

## 行为修正：Anthropic 输出上限自适应

客户端未传 `max_output_tokens` 时，Anthropic Messages 请求默认使用 512K
输出预算。不同托管方对同一模型的上限不同（例如百度 OneAPI 的 GLM-5.3 为
256K）。当 provider 以 4xx 拒绝默认预算、并在错误信息中给出上限时，网关用该
上限重试一次，并在本进程内记住该 provider 模型的上限，之后的请求直接使用。
客户端显式传入的上限从不改写；上下文窗口溢出等其他错误不会被当作输出上限。

