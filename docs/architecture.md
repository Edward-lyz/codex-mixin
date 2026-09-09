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

依赖方向是单向的：`cli -> application -> provider/config/catalog/clients`，
`server -> gateway -> upstream -> provider/protocol`。`gateway` 与 `fusion`
不再引用 `server` 或 `AppState`。

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
