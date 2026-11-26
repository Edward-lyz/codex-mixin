# Codex Mixin 架构

本文档描述模块职责、依赖方向与组件模型。静态边界由 `scripts/check_arch.sh`
强制，在 CI 的 fmt 之后执行。

## 模块职责

| 层级 | 目录 / 模块 | 职责 | 允许依赖 |
|---|---|---|---|
| **L0 `core/`** | `access`、`error`、`usage`、`images`、`protocol`、`provider` | 纯内核与领域规则：错误边界、访问凭据、计量、图片规范化、协议 DTO/转换/流式 FSM、Provider 规范/Adapter/发现/探测 | 仅层内下级，不依赖上层 |
| **L1 `infra/`** | `config`、`clients`、`ech`、`pi_plugins`、`web_search` | 本地存储与外部基础设施：配置持久化与迁移、客户端配置文件渲染、ECH 客户端、Pi 插件、联网搜索探测 | `core` |
| **L2 `runtime/`** | `catalog`、`context`、`upstream`、`gateway`、`fusion`、`benchmark`、`server` | 在线网关数据面：目录缓存、长上下文压缩、出站传输、路由/故障转移、多模型融合、压测、HTTP/WS 入口 | `core`、`infra` |
| **L3 `app/`** | `app`（对外别名 `application`） | 离线管理用例编排：provider 管理/刷新、客户端接入、诊断、上报；`OperationError` 错误语义 | `core`、`infra`、`runtime`（不依赖 `server`） |
| **二进制外壳** | `cli/`（子命令位于 `cli/commands/`） | clap 参数、交互输入、输出渲染、进度、退出码 | `app` 及公开库类型 |
| **平台层** | `crates/platform` | 全部操作系统差异：路径、进程组、权限与锁、开机自启、桌面通知、终端窗口、代码签名 | 标准库与平台命令 |
| **GUI 壳** | `desktop` / `panel` | 平台壳层；渲染 UI 并调用稳定 CLI contract | CLI 子进程及公开 core 类型 |

依赖方向严格单向下三角：`cli -> app -> runtime -> infra -> core`。`gateway` 与 `fusion` 不引用 `server` 或 `AppState`。

## 平台无关的核心

`crates/platform` 之外的生产代码（库、CLI、Web 面板）不含任何平台条件编译、平台
工具名、平台路径或平台环境变量。核心只表达意图，例如“把子进程放进独立进程
组”“以当前用户私有权限写文件”“登录后自动启动网关”“显示一条桌面通知”
“在终端窗口完成扫码登录”，由 `platform` 在 macOS（launchd、plutil、
codesign、osascript）、Linux（systemd --user、notify-send）与 Windows
（计划任务、taskkill、ACL、Windows Terminal）上分别实现。

`scripts/check_arch.sh` 扫描 `src/` 与 `panel/` 中 `crates/platform` 以外的代码，
发现 `cfg(unix|windows|target_os)`、`std::os::*`、`.exe`、`launchctl`、
`USERPROFILE`、`/usr/`、权限位等平台片段即失败。

## 壳与 CLI 的契约

三个 UI 壳均位于仓库顶层。`src/main.rs` 是唯一的 composition root：它把
CLI 解析出的抽象交互入口连接到 `panel`（axum Web 壳，静态资源内嵌），core 和 CLI 模块不引用任何具体壳。
`panel` 的「上下文」页读取 `/api/context`（`context stats --json` 与 `context config get --json`），
并通过白名单动作 `context_configure` 映射为 `context config set`。
macOS、Windows 与 Web 面板壳通过带 `--no-tui` 的 CLI 子进程访问同一组用例与 JSON
contract，平台 UI 不复制 provider、gateway 或 client integration 业务规则，
也不读写 CLI 状态目录中的任何文件。壳自己的数据（诊断日志、额度展示缓存、
图标缓存）放在各自的原生目录（macOS 为 `~/Library/Application Support/Codex Mixin`，
Windows 为 `%LOCALAPPDATA%\CodexMixin`）。

| 命令 | 用途 |
|---|---|
| `interface --json` | 协议版本、CLI 版本、状态/配置/日志路径与能力开关 |
| `--json-errors` | 失败时 stderr 最后一行为 `{protocol_version, error:{code, message, committed, stage}}` |
| `service status\|ensure --json` | 网关状态；`ensure` 按版本、自启设置与服务定义收敛到唯一的当前网关 |
| `service start\|stop\|restart --managed --json` | 启停；自启已开启时经系统服务管理器，否则为后台 daemon |
| `service autostart enable\|disable\|status --json` | 网关登录自启（launchd / systemd --user / 计划任务） |
| `config apply` | 保存后的整体生效：无 provider 时停网关，否则重启并同步 Codex 与各客户端目录 |
| `route list\|get\|set\|delete --json` | 模型路由（融合、定时、故障转移），见 [model-routing.md](model-routing.md) |
| `route models --json` | 路由可选模型（含 `official:<slug>`）；`fusion models --json` 为兼容别名 |
| `context config get\|set --json` | 长上下文引擎开关与阈值；只写配置，不启动网关 |
| `context status\|sessions\|export --json` | 会话 revision、分层计数、可压缩范围与原始归档导出 |
| `context stats --json` | 每会话与总计的压缩节省量、视图 token、上游缓存命中 |
| `connect ducx --json` | 安装并登录托管 DUCX；无终端时由 CLI 打开终端窗口扫码，返回可执行文件路径 |

启停规则由 Rust 负责：自启定义存在即表示自启开启；启动、停止、重启、升级与
服务定义迁移都不改变这一设置；GUI 超时不代表 CLI 操作已停止。

## 组件模型

- `upstream::UpstreamAccess`：HTTP client、官方认证缓存、DUCX 运行时，提供
  provider/官方发送与认证；`server` 与 `gateway` 都通过它发请求。
- `gateway::GatewayExecutor`：路由解析、请求计划、单请求流式执行、缓存观测；
  是 fusion 与 server 唯一的网关执行入口。
- `catalog::CatalogService`：模型枚举、目录响应缓存与模型窗口解析；
  server 和 gateway 共享同一实例及元数据缓存。
- `benchmark::resolve_targets`：结合 `CatalogService` 与筛选条件解析压测目标，避免 `catalog` 反向依赖 `benchmark`。
- `context::ContextEngine`：按会话同步原始消息、渲染压缩视图、校验并提交压缩、
  提供检索、导出与压缩/缓存统计；网关内部回路通过它执行四个 context 工具。
- `server::AppState`：纯组合对象，持有上述组件与 benchmark/图片路由等
  handler 所需状态，不再实现发送、认证或目录业务。

## 核心抽象与扩展规范

Provider 端点解析（`EndpointSpec` + `WirePolicy`）、Agent 注册表（`AgentManifest` + `FrameworkSpec`）与协议流式两层状态机（`MapperState` + `ResponsePhase`）的设计思路、UML 图与开发准入规则见 [Provider、Agent 与协议流式抽象设计规范](abstraction-architecture.md)。

## 提交语义

provider 管理用例把网络操作放在锁外，先提交主配置，再执行缓存失效、发现、
探测与同步。提交后的失败返回 `application::error::OperationError::AfterCommit`
并携带失败阶段，提示“配置已保存，某阶段失败”，而不是暗示什么都没发生。
失败退出码保持不变；后续阶段可通过 `provider refresh` 等入口重试，不会重复
创建 provider，也不会吞掉错误。

## 架构检查

`scripts/check_arch.sh` 是粗粒度静态源码检查，不宣称完整的 Rust 语义分析；
clippy 与测试仍是权威检查。当前规则：

- 核心请求路径（gateway/upstream/provider/protocol/fusion/catalog/config/
  benchmark/web_search/images/application）不得引用 `crate::server`、
  `crate::cli` 或 `AppState`。
- 协议转换与编解码模块不得执行网络发送；入站 Body 读取在 `server`，JSON
  序列化与发送在 `upstream`。
- 库用例不得使用 clap/indicatif/ratatui/console/crossterm 或终端打印。
- 底层模块不得引用 `FusionEngine` 或网关执行器。
- `server` 不得引用 `crate::cli`。
- core/CLI 不得引用具体 `panel` 壳；面板不得访问 CLI 私有实现。
- `crates/platform` 以外的代码不得出现平台条件编译、平台工具名、平台路径或平台
  环境变量。
- MIT 公理设计双层下三角依赖矩阵门禁：`scripts/check_arch.sh` 同时校验 4 层宏边界（禁止 L(i) → L(j), j > i）与 19 个库模块的 Tarjan SCC 零环约束。
