# PLAN：网关与菜单栏内存压缩

状态：继续优化开启 context 的并发长会话；早期低于 10 MB 的测量不能代表此负载。菜单栏 UI < 50 MB 目标暂缓（控制台打开时 63 MB，未达标）。分支 codex/perf-disconnect-openai。

## 目标
- 网关 `codex-mixin start` 常驻 phys_footprint < 10 MB；菜单栏 UI < 50 MB。
- 展示与交互不变；每项改动附测量数据。

## 第一步：按需查询每轮用量（src/gateway/turn_usage.rs）
- 现状：`TurnLedger::load` 启动时把最近 5000 行读进 `VecDeque<TurnUsage>`，每行多个 String，
  常驻约 3–4 MB，`record` 再 clone 一份进内存。`/v1/usage/turns` 已在 blocking 线程里调用 `query`。
- 改后：内存里不再保存行；`query` 直接在 usage.sqlite3 上按 `at_ms DESC, id DESC` 过滤 since/provider，
  逐行读取直到 limit；共用已有的写连接并把页缓存限制在 256 KB。
- 风险：刚完成的一轮在异步落盘前查询不到（毫秒级）；数据库不可读时查询返回空并记 warning。

## 第二步：models.dev 目录不常驻（src/provider/resolver.rs）
- 现状：解析后的目录（235 个 provider、8930 个模型，磁盘 6 MB）一直挂在 `MODELS_DEV_CATALOG_CACHE` 里。
- 改后：缓存只保留 `Weak<ModelsDevCatalog>`，发现过程结束就释放；再次发现时从已校验的磁盘副本重新解析。
- 风险：每次发现多一次解析（毫秒级）。

## 测量（phys_footprint，MallocSpaceEfficient=1，真实配置副本）
| 进程 | 改前（0.7.7） | 改后 |
|---|---|---|
| 网关 `start`（含模型刷新线程） | 77 MB（峰值 197） | 8.1 MB；每个接口各调 30 次后 8.2 MB |
| 网关 `serve --no-client-sync` | 19 MB | 3.7–3.9 MB |
| 网关 `start`，真实流式负载（baidu-oneapi `DeepSeek-V4.1-Flash`，`/v1/responses` stream） | — | 空闲 5.7 MB；20 个流式请求（4 并发）6.2–7.0 MB；4 并发 180 KB 输入 7.1–7.5 MB；之后静置 8.4–8.5 MB |
| 菜单栏（控制台已关闭） | 51 MB | 39 MB |
| 菜单栏（控制台打开） | — | 63 MB，关闭后回到 39 MB |

真实负载测试使用独立 HOME、`CODEX_GATEWAY_SERVICE_ID` 与独立端口 `127.0.0.1:18766`，不触碰用户正在运行的网关。启动瞬时峰值（`phys_footprint_peak`）约 44–47 MB，来自启动时的模型目录解析，稳态和负载下均低于 10 MB。

## 第三步：WebSocket 与 context 的临时副本（src/server/responses_ws.rs、src/context/store.rs）
- 官方 WS 路由：`effective_official_cache_body` 为前缀观测复制整份历史，原先这个值活到上游往返结束；
  改为在观测的局部块里创建并释放。
- context 引擎 `history_shape`：原先 `Store::history` 把会话所有指纹（规范化后的完整 JSON，最大会话约 10 MB）
  一次 collect 再比前缀；改为新增 `Store::history_prefix`，逐行比较，指纹不再同时驻留。
- 风险：前缀比较语义不变（遇到第一个不同或任一侧结束即停）；`history_len` 改用 COUNT 查询。

## CLI 内存清单（测量值均为 phys_footprint，独立实例 127.0.0.1:18766）
| 所有者 | 生命周期 | 上限 | 处理 | 实测 |
|---|---|---|---|---|
| `TurnLedger`（src/gateway/turn_usage.rs） | 进程 | 原 5000 行 | 按需查 SQLite | 已做，见第一步 |
| models.dev 目录（src/provider/resolver.rs） | 原进程 | 6 MB 磁盘 | `Weak`，用完释放 | 已做，见第二步 |
| `TokenUsageAggregator` 日表（src/gateway/cache_usage.rs:131） | 进程，按天增长 | 每天 × provider × model 一行 | 未改 | 真实库 694 行：清空日表 7.5 MB，完整 694 行 5.3 MB，差异在噪声内，暂不改 |
| WS 连接历史（responses_ws/custom.rs、official.rs） | 连接 | 随轮数增长 | 有协议用途，未改 | 8 轮 × 180 KB 输入：改前 10 MB，断开回到 8.9 MB；改后 8.9 MB，断开 7.7 MB |
| WS 前缀观测副本（responses_ws.rs） | 原为整轮请求 | 一份历史 | 局部释放 | 并入上一行数据 |
| context 历史指纹（context/engine.rs history_shape） | 单次请求 | 会话全部指纹 | 逐行比较 | 最大会话 3254 条、指纹约 10 MB；独立实例未开 context，收益为推算 |
| 共享 HTTP Client | 多处每次新建 | — | 未改 | 假设，未测 |

## 第五步：context 每轮 begin_turn 不再复制整段历史（src/context/engine.rs、src/context/store.rs）
- 改前：`begin_turn` 先为全部历史生成两份 `Vec<String>`（规范化指纹、JSON 编码），`sync_history` 再把全部归档指纹读进内存比较；
  `compute_protection` 又把所有归档消息重新解码一遍。
- 改后：`sync_history` 用 `history_prefix` 逐行比较，只给分叉点之后的新增条目生成指纹和编码；`compute_protection` 借用 `render_view` 已解码的条目。
- 实测（隔离实例，真实 context.db 副本，1808 条历史、4.4 MB 请求体，context 已开启并命中 `prepared context view`，0.5 秒采样）：

| 阶段 | 改前 footprint / 已分配峰值 | 改后 |
|---|---|---|
| 首次归档整段会话 | 36.9 MB / 36.9 MB | 28.9 MB / 31.1 MB |
| 稳态续轮（只追加 2 条） | 21.6 MB / 63.1 MB | 15.2 MB / 33.1 MB |
| 请求结束 30 秒后 | 13.4 MB | 11.6 MB |

## 后续候选
- tokio blocking 线程池上限与线程栈（假设，未测）。
- 菜单栏：暂缓；如恢复，仅在测量确认收益时改动，不改变外观。

## 并发长会话：归档后释放原始历史

2026-11-05，在真实 release CLI 上用 `scripts/measure_context_memory.mjs` 测量。
每个并发档重新启动独立网关和数据库，开启 context，每会话 1002 条消息、
3,988,976 字节输入；本地 Responses 上游先发送 SSE 首事件，再保持所有请求，
取 5 次 `vmmap -summary` 样本的最大值。此项是 HTTP 存活内存测量，
不代表真实模型推理、WS 长连接或已压缩历史的全部表现。

- 原始输入归档一次后即释放，内部工具轮次只读取已提交的当前视图，不再用原始历史重新同步。
- 组装上游请求时移动 `Vec<Value>`，不在整轮推理期间额外持有当前视图副本。
- 复用已有 SQLite 连接和标准容器；不把大 JSON 树放进进程级对象池。

| 并发 | 改前存活堆 / footprint | 改后存活堆 / footprint | 改后复测 |
| --- | --- | --- | --- |
| 1 | 14.2 / 32.1 MiB | 3.4 / 26.8 MiB | 3.4 / 26.1 MiB |
| 4 | 47.9 / 81.8 MiB | 5.0 / 17.8 MiB | 5.0 / 16.8 MiB |
| 8 | 92.9 / 121.6 MiB | 7.0 / 58.9 MiB | 7.0 / 56.3 MiB |

8 并发存活堆减少约 92.5%；准备阶段 footprint 峰值从 135.9 MiB 降到
109.9–112.2 MiB，尚未消除解析和发送时的短时重叠。footprint 与结束后的
回落受分配器驻留页影响，因此同时记录存活堆，不能用某次空闲采样代替并发验收。

复现：在仓库根设置 `EVAL_BINARY` 指向待测 release 二进制，运行
`node scripts/measure_context_memory.mjs`。脚本只访问本地受控上游，自动清理
自身临时目录和子进程，不修改真实服务、配置或会话。

## HTTP 与 WebSocket context 长历史（2026-11-05）

HTTP 侧在首次同步后释放原始历史；内部工具轮次从已提交的当前视图继续，组装上游请求时移动 `Vec<Value>`，避免整段执行期间再留一份历史副本。WebSocket 侧只对启用 context 且有 context session 的非 Fusion 自定义协议路由生效：上游流启动后立即释放返回的原始请求 body；成功完成后，连接状态只保留一个带认证的 checkpoint 项和本轮完成输出。checkpoint 绑定用户可见的 model slug、session revision 与 history anchor；下一轮通过现有 checkpoint 输入路径恢复归档视图。无 context 与 Fusion 路径继续保留原连接历史。

测量使用 `scripts/measure_context_memory.mjs` 和 macOS `vmmap -summary`，before 为 `/tmp/codex-mixin-context-before`，after 为当前 release CLI。每档并发重新启动隔离网关和数据库；每个会话输入 1002 条消息、3,988,976 字节 JSON。WS 输入为每条连接设置唯一 `prompt_cache_key`，并从 SQLite 检查 session 数等于并发数，确保实际经过 context。受控本地上游同时挂起所有流；WS 的 `active` 在上游仍挂起时采样，`postOpen` 在请求完成而 socket 仍打开时采样，`postClosed` 在关闭 socket 后采样。表中数值为 `MALLOC ZONE TOTAL ALLOCATED` 字节（1 / 4 / 8 并发）；它比 footprint 更接近当前分配量，但不是模型推理的端到端内存结果。

| 传输 / 状态 | 改前 | 改后 |
| --- | --- | --- |
| HTTP，所有请求保持期间 | 14,889,779 / 50,226,790 / 97,412,710 | 3,598,336 / 5,221,376 / 7,385,088 |
| WS，active，上游挂起 | 24,117,248 / 87,241,523 / 171,232,461 | 5,131,264 / 17,720,934 / 34,498,150 |
| WS，postOpen，socket 保持打开 | 13,002,342 / 42,362,470 / 81,474,355 | 5,190,656 / 17,720,934 / 34,393,293 |
| WS，postClosed，socket 已关闭 | 3,193,856 / 3,350,528 / 3,543,040 | 1,005,568 / 1,191,936 / 1,388,544 |

HTTP 从 1 到 8 并发的分配量斜率由约 11.25 MiB/session 降至 0.516 MiB/session（约降低 95.4%）。WS active 在 8 并发从 171,232,461 降至 34,498,150 字节（约降低 79.9%）；postOpen 从 81,474,355 降至 34,393,293 字节（约降低 57.8%）。postClosed 仍有分配器驻留页，因此不能按该值推断进程 footprint 会同步下降。

边界：输入历史比普通会话大，provider 是本地受控桩，不执行真实模型推理；结果衡量历史保留和请求处理期间的内存，不覆盖真实网络波动、不同 payload 分布或长时间运行的全部状态。WS 的 active/postOpen 仍包含每个请求处理与连接所需的基础内存；Fusion 和未启用 context 的连接没有应用 checkpoint 历史压缩。`phys_footprint` 受分配器页驻留影响、采样波动较大，故表格比较分配量，不能据此承诺常驻 footprint 按相同比例下降。

## 停止续聊与六小时原文保留

- 自定义协议 WS 在等待会话锁、首事件或流式响应时持续接收 Close/EOF；断开即取消整轮请求并释放 OS 会话锁。待处理请求最多 16 条且合计不超过请求大小限制。
- 后台每 300 秒处理最多 8 个会话，跳过持锁会话。闲置超过六小时后，仅清空当前摘要覆盖的原文和旧响应输出，保留引用、指纹、历史位置、摘要、当前未压缩消息和最新响应输出。原有 30 天闲置会话清理保留用量统计。
- 新 checkpoint 和响应映射记录历史长度与末条引用，续聊直接恢复有效视图，不依赖已过期原文。旧格式在原文仍在时兼容；需要的原文已过期时明确报错。解压先检查过期引用，失败不修改 revision；搜索标明结果不完整。
- 清理和正常请求使用同一 OS 会话锁。迁移回填只执行一次；增量 vacuum 完整消费每页结果，并以每轮 8192 页限额配合被动 WAL checkpoint。旧库若未启用 incremental auto-vacuum，本轮不会在启动路径执行全库 VACUUM。
- 真实归档副本经过两轮清理：244 个会话、700 个摘要块、61,867 条历史引用均保留；过期 5,124 条消息原文及 2,230 条旧响应输出。消息正文从 364,356,609 降至 345,403,738 字节，响应正文从 114,175,251 降至 97,227,911 字节；数据库文件从 619,696,128 降至 593,604,608 字节。当前未压缩消息中没有过期正文，`integrity_check` 为 `ok`。此验证仅清理副本。
- 验证：完整 Rust 集成测试 246 通过、7 项原有忽略；默认及 client-auth 严格 Clippy、架构检查、client-auth 定向测试、multi-provider 和 prompt-cache E2E 通过。macOS App 使用已安装的 MacOSX26 SDK 构建并通过签名验证；未重启已安装服务。
