# Codex Mixin 架构

本文档描述本次 SOLID 重构后的模块职责、依赖方向与组件模型。静态边界由
`scripts/check_arch.sh` 强制，在 CI 的 fmt 之后执行。

## 模块职责

| 模块 | 职责 | 允许依赖 |
|---|---|---|
| `cli`（二进制） | clap 参数、交互输入、输出渲染、进度、退出码 | `application` 及公开类型 |
| `application` | provider 管理、刷新、客户端接入等用例；`OperationError` 错误语义 | `provider`/`config`/`catalog`/`clients` |
| `server` | HTTP/WS 解析、连接状态、请求分发、响应封装 | `gateway`/`fusion`/`catalog`/`benchmark`/`upstream`/`provider` |
| `gateway` | 路由、请求计划、单次执行、缓存观测 | `upstream`/`provider`/`protocol`/`images`/`web_search` |
| `fusion` | 多模型编排、工具执行协调、结果整合 | `gateway`（普通执行能力） |
| `upstream` | 请求准备、上游传输、认证运行时、协议相关重试 | `provider`/`protocol`/`config` |
| `catalog` | 目录生成、来源加载、目录缓存 | `provider`/`web_search`/`protocol` |
| `provider` | 定义、校验、preset、能力与路由元数据 | 纯规则，不依赖上层 |
| `protocol` | 请求转换、SSE 编解码、事件映射、协议数据 | 纯数据，不发送网络 |
| `config` | 配置模型、迁移、校验、持久化 | 锁与原子替换 |
| `clients`（新增） | 各编码客户端配置渲染、安装、同步、卸载 | 不依赖 server |
| `platform` | 全部操作系统差异：路径与 home 变量、进程组与进程树、文件权限与锁、开机自启服务、桌面通知与桌面 App、终端窗口、代码签名、Codex CLI 位置、包目标名 | 标准库与平台命令 |
| `macos` / `tui` / `windows` | 平台壳层；渲染 UI 并调用稳定 CLI contract | CLI 子进程及公开 core 类型 |

依赖方向是单向的：`cli -> application -> provider/config/catalog/clients`，
`server -> gateway -> upstream -> provider/protocol`。`gateway` 与 `fusion`
不再引用 `server` 或 `AppState`。

## 平台无关的核心

`src/platform` 之外的生产代码（库、CLI、TUI）不含任何平台条件编译、平台
工具名、平台路径或平台环境变量。核心只表达意图，例如“把子进程放进独立进程
组”“以当前用户私有权限写文件”“登录后自动启动网关”“显示一条桌面通知”
“在终端窗口完成扫码登录”，由 `platform` 在 macOS（launchd、plutil、
codesign、osascript）、Linux（systemd --user、notify-send）与 Windows
（计划任务、taskkill、ACL、Windows Terminal）上分别实现。

`tests/platform_neutral_core.rs` 扫描 `src/` 与 `tui/` 中 `src/platform`
以外的生产代码，发现 `cfg(unix|windows|target_os)`、`std::os::*`、`.exe`、
`launchctl`、`USERPROFILE`、`/usr/`、权限位等平台片段即失败。测试专用代码
（`#[cfg(test)]` 项与 `tests.rs`）可以按平台准备 fixture。

## 壳与 CLI 的契约

三个 UI 壳均位于仓库顶层。`src/main.rs` 是唯一的 composition root：它把
CLI 解析出的抽象交互入口连接到 `tui`，core 和 CLI 模块不引用任何具体壳。
macOS 与 Windows 壳通过带 `--no-tui` 的 CLI 子进程访问同一组用例与 JSON
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
| `fusion models --json` | Fusion 可选模型（含 `official:<slug>`） |
| `connect ducx --json` | 安装并登录托管 DUCX；无终端时由 CLI 打开终端窗口扫码，返回可执行文件路径 |

启停规则由 Rust 负责：自启定义存在即表示自启开启；启动、停止、重启、升级与
服务定义迁移都不改变这一设置；GUI 超时不代表 CLI 操作已停止。

## 组件模型

- `upstream::UpstreamAccess`：HTTP client、官方认证缓存、DUCX 运行时，提供
  provider/官方发送与认证；`server` 与 `gateway` 都通过它发请求。
- `gateway::GatewayExecutor`：路由解析、请求计划、单请求流式执行、缓存观测；
  是 fusion 与 server 唯一的网关执行入口。
- `catalog::CatalogService`：模型枚举、benchmark 目标、目录响应缓存。
- `server::AppState`：纯组合对象，持有上述组件与 benchmark/图片路由等
  handler 所需状态，不再实现发送、认证或目录业务。

## 提交语义

provider 管理用例把网络操作放在锁外，先提交主配置，再执行缓存失效、发现、
探测与同步。提交后的失败返回 `application::error::OperationError::AfterCommit`
并携带失败阶段，提示“配置已保存，某阶段失败”，而不是暗示什么都没发生。
失败退出码保持不变；后续阶段可通过 `provider refresh` 等入口重试，不会重复
创建 provider，也不会吞掉错误。

## 行为修正

- 客户端密钥同步不再从全局命令分发执行；接入、密钥变更、服务启动与明确
  修复流程才会同步，`doctor` 报告损坏的托管配置，无关查询不受损坏客户端影响。
- Anthropic Messages 使用与其他协议一致的模型级端点（`api_url_for_model`），
  保留 Baidu 路由、AWS SigV4 签名与 DUCX 头部。
- 提交后失败准确表达阶段与“配置已保存”。

详见 `docs/behavior-changes.md`。

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
- core/CLI 不得引用具体 `tui` 壳；TUI 不得访问 CLI 私有实现。
