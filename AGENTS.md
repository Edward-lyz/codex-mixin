# Codex Mixin 项目守则

## 适用范围与执行顺序

本文件适用于整个仓库。进入子目录时同时遵守沿路径各级 `AGENTS.md`；子级只补充本目录约束，不解除父级的安全、分层和验证要求。规范描述新增与修改代码的约束，不证明历史代码已全部达标。遇到既有越界，说明事实并局部收敛，不以文档任务为由重构整个项目。

仅在职责、依赖边界或开发方式明显不同的目录建立 `AGENTS.md`。普通子模块、资源、语言包、fixture、生成文件和配置目录继承最近的上级规范，不逐目录复制规则。新增目录默认继承；只有父级无法清楚表达其独有约束时才增加规范。缓存、构建产物和下载的第三方工具不创建规范。

## 产品原则：CLI 是核心，GUI 是外壳

Codex Mixin 的核心交付是可独立运行的 Rust CLI 与本地网关，连接官方服务及 OpenAI Chat Completions、Anthropic Messages 等兼容 provider。macOS 菜单栏 App、Windows Flutter App 和 TUI 是同一能力的交互外壳，不是独立业务实现。

- 新增业务必须先提供可在 `--no-tui` 下使用的 CLI 入口及可验证的 Rust 用例；自动化调用需要稳定 JSON 输出与结构化错误。无 GUI 环境也必须能配置、运行、诊断和恢复。
- “CLI 是核心”不等于把业务都塞进 `src/cli/`。参数、输入、渲染与退出码属于 CLI；业务规则、提交顺序和可复用能力属于 `application` 及下层模块。
- GUI/TUI 负责输入收集、页面状态、进度和错误展示，通过 CLI contract 执行业务。不得复制 provider 校验、模型路由、配置迁移、鉴权、Fusion、客户端接入或网关启停策略。
- GUI 可管理自己的窗口、托盘、登录启动、图标缓存和诊断日志。GUI 登录启动与网关自启是不同语义；后者始终由 CLI/core 管理。
- GUI 不读写 CLI 状态目录来实现业务，不猜测其布局；通过 `interface --json` 获取版本、能力和路径。按 CLI 返回路径打开日志/目录是展示动作，不是直接修改状态。

## 抽象层级与依赖方向

控制面：`CLI -> application -> provider/config/catalog/clients/platform`。
请求面：`server -> gateway -> upstream -> provider/protocol`；`server -> fusion -> gateway`。
这是主路径而非强行划分的全局数字层级。允许下面列出的共享能力与组合入口，不允许反向依赖或为绕过边界机械增加一层转调。

| 目录/模块 | 承载的抽象 | 边界 |
| --- | --- | --- |
| `src/main.rs` | 二进制 composition root | 连接 CLI 抽象交互入口与具体 TUI；不放业务 |
| `src/cli/` | 核心交付接口 | 调用用例、组织输入输出；历史适配代码不成为继续堆积业务的理由 |
| `src/application/` | 完整业务用例 | 接收具体依赖、返回领域结果；无终端打印或 UI 依赖 |
| `src/server/` | 入站 HTTP/WS adapter | 认证、解析、请求/连接生命周期、响应封装；状态对象仅组合组件 |
| `src/gateway/` | 普通单请求执行 | 路由、请求计划、执行、缓存观测；不依赖 server/CLI/Fusion 编排 |
| `src/fusion/` | 多模型用例编排 | 复用 gateway 执行；不另造 provider 发送栈 |
| `src/upstream/` | 上游 transport | 请求准备、认证运行时、发送、协议相关重试；不反向依赖业务编排 |
| `src/protocol/` | 协议与流状态 | 纯转换/编解码，不发网络请求、不读配置、不运行子进程 |
| `src/provider/` | Provider 定义及能力适配 | 规则与元数据；现有 discovery/auth/probe 可有受限 I/O，不负责网关编排 |
| `src/config/` | 配置模型与存储 | 版本、校验、迁移、加密、锁、原子替换；不发请求或呈现 UI |
| `src/clients/` | 编码客户端配置 adapter | 显式路径与网关数据，保留非托管配置；流程次序交给 application |
| `src/platform/` | OS adapter | 路径、权限、进程、终端、系统服务、安装与签名的平台差异 |
| `src/catalog/` | 模型目录 | 来源合并、生成、缓存；不参与请求发送或 UI |
| `src/benchmark/`、`src/web_search/`、`src/images/`、`src/ech/` | 专用能力 | 按 `src/AGENTS.md` 处理探测、媒体或连接；不引用上层控制面 |
| `macos/`、`windows/`、`tui/` | 表现与平台壳 | 通过真实 CLI 访问同一能力，不形成第二套业务状态 |

`src/platform/` 以外的 Rust 生产代码不得出现 OS 条件编译、原生权限位、系统工具名或平台路径；意图通过 platform API 表达。测试 fixture 可以按 OS 准备环境。原生 GUI 的窗口/托盘行为留在自身平台壳。

可见性默认 private，跨兄弟模块按最小需要使用 `pub(super)` / `pub(crate)`。新增对外 `pub` API、protocol 字段或扩展点先说明使用方与契约并取得确认，不为测试随意公开内部实现。

### 何时提取抽象

- 只调用一次且 1–3 行的机械 helper 保持内联；两处相似实现先保持局部。三处以上重复、严格共同变化、领域概念、复杂边界或明确测试入口才提取。
- 使用已有具体组件，只有真实多实现需求才引入 trait、registry、strategy、factory 或 plugin。一个实现不需要继承树或“未来扩展框架”。
- 不创建无语义的 service/controller/helper 层。每个边界应拥有明确输入、输出、失败语义或资源生命周期。
- 新抽象说明删除它会丢失什么实际能力、验证路径及成本；性能优化给出前后行为和量化依据。

## 编码、失败与安全

- Rust 保持 `unsafe_code = "forbid"`。生产路径不使用 `.unwrap()`、`.expect()`、`panic!`；使用 `?` 和 `anyhow::Context` 保留原始原因。测试可用这些方法表达必须满足的前提。
- 沿用相邻命名、结构及已有依赖。Rust 源码保持 ASCII（非 ASCII 展示文本用现有方式表达）；资源和本规范使用 UTF-8。注释解释 why；重复或有领域含义的字面值提取常量/enum。
- 用 early return、明确 enum 和短作用域保持主路径清楚。不为缩短 diff 吞错误，不把异常返回为空结果、mock 或成功状态。
- HTTP 边界使用 `GatewayError`；内部错误不向客户端暴露路径、拓扑、认证和错误链。库日志使用 `tracing`，原始错误链在内部经脱敏后保留；库不打印终端输出。
- fail fast。兼容、协议重试及产品降级必须有明确触发条件、限额、可见状态和测试，不允许 hidden fallback。保留项目已定义的 ECH/协议降级语义，不把“禁止静默降级”误写成禁止所有显式重试。
- provider 变更先提交主配置，再失效缓存、发现、探测和同步；网络不占配置锁。提交后失败保留 `OperationError::AfterCommit` 的阶段，不能向用户宣称未保存或已回滚。
- trust boundary 校验 URL、路径、请求大小、协议字段、归档条目及子进程输入。进程参数用参数数组，不拼接含用户输入的 shell 命令。
- API Key、Token、Cookie、签名头、配置解密结果不得提交、打印或进入截图/fixture。GUI 密钥经既有安全输入机制（如 `@env:NAME`）传给 CLI，不写命令行或动作日志。
- Base64 备份不是加密。导入导出保留明确风险提示、校验及当前用户私有权限；不要覆盖无关用户配置或无限制清理文件。

## 性能与资源生命周期

- 以树莓派级硬件为基线，网关转发为热路径。SSE/WS 增量处理，不无界缓冲完整响应；限制队列、缓存、任务并发和媒体解码开销。
- 复用 HTTP client、`bytes` 与现有扫描模式；避免逐事件重复解析、全量 clone 和重复 I/O。
- async 不直接执行阻塞文件/进程等待或 CPU 密集媒体处理；沿用既有隔离方式，新增阻塞路径明确移出 runtime worker。
- 锁只覆盖状态变更，不跨网络 await。取消、超时、退出必须收敛相关任务、连接和子进程；GUI 超时不代表 CLI 操作已回滚。
- 保持 release 的 `lto = "thin"` 与 `codegen-units = 1`。不把每次编译后的 `cargo clean` 当门禁；仅在有空间压力或明确需要时清理已确认的项目产物，避免破坏增量缓存与并行任务。

## 验证与交付

实现前读相邻源码、契约和测试；若提供 LSP，优先用于定位引用。没有可用 LSP 接口时使用文件搜索与原生读取，不宣称执行了语义分析。

验证命令默认从仓库根运行，Windows Flutter 命令另标工作目录。修 bug 先写复现并观察失败，再修复并观察通过；完成工作时执行最终检查。

Rust/分层改动的门禁：

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- \
  -D clippy::cognitive_complexity \
  -D clippy::redundant_closure_for_method_calls \
  -D clippy::redundant_pattern_matching -D warnings
./scripts/check_arch.sh
./scripts/test_check_arch.sh
cargo test --locked --all-targets
```

CI 还运行 `client-auth` 的严格 Clippy 和全目标测试，以及 `./scripts/e2e_prompt_cache.sh`。鉴权、feature 条件、请求转换、流式和缓存前缀改动必须覆盖相应路径；窄范围测试不替代最终 CI 门禁。具体 GUI、资源、打包检查见目录规范。

文档/规范单独改动至少检查 `git diff --check`、本地链接/命令存在性、规范继承与架构检查；没有源码或构建配置变化时可不重编译 Rust/GUI，但汇报中必须明确未运行哪些检查及原因。

单测就近放 `#[cfg(test)] mod tests` 或模块 `tests.rs`；跨组件行为放 `tests/`。允许确定性的本地测试服务器与 fixture，但必须驱动真实生产路径，不能以 mock 结果冒充线上成功或删掉安全断言。

`check_arch.sh` 是粗粒度文本门禁，不是完整语义检查。不要通过别名、重导出或修改断言规避边界；扩大门禁时补充其负例测试。

一次改动聚焦一个目标，保留用户已有工作。验证后按最小闭包 commit，不 push 或触发发布，除非用户明确要求。汇报修改内容、实际命令、结果、跳过项和剩余限制，不用“应该通过”代替实际输出。
