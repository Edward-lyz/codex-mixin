# PLAN：网关与菜单栏内存压缩

状态：CLI 网关阶段已完成；菜单栏 UI < 50 MB 目标暂缓（控制台打开时 63 MB，未达标）。分支 codex/perf-disconnect-openai。

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
