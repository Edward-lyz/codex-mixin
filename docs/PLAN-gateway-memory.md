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

## 停止续聊与原文永久保留

- 自定义协议 WS 在等待会话锁、首事件或流式响应时持续接收 Close/EOF；断开即取消整轮请求并释放 OS 会话锁。待处理请求最多 16 条且合计不超过请求大小限制。
- 取消六小时清空原文、旧响应输出和 30 天删除闲置会话的策略，也不自动删除失去当前引用的历史消息。模型摘要压缩继续工作；原文在 SQLite 内长期保留。没有 `archives/`，不改写客户端会话文件。
- checkpoint 和响应映射继续记录历史长度与末条引用，以有效视图续聊；旧版本已经清空的正文仍显式标记缺失。解压缺失原文失败时不修改 revision；搜索标明结果不完整。
- 后台按同一 OS 会话锁迁移旧正文：每轮最多 8 个会话、每会话最多 128 条消息和 128 条回复，以 4 MiB 原文字节为本轮目标（单条大正文作为完整记录处理）。有积压每 5 秒继续，无积压每 300 秒检查。这些是维护工作量，产品并发没有设为 8。

## 单库无损压缩：第一阶段

- 保持表关系、引用、摘要和续聊锚点；消息、回复采用 SQLite TEXT/BLOB 混合存储。`body_codec=0` 为 JSON，`1` 为 zstd level 3；不足 256 字节或压缩未变小则保留原文。记录 UTF-8 原始字节数与 SHA-256，替换前验证往返一致，读取时校验长度和哈希。旧 TEXT 无需一次性转换即可读取，新记录直接按新格式写入。
- 解码器单条原文上限 256 MiB，按需解码，不常驻完整历史或增加全局正文缓存。搜索逐条解码，预览和精简导出通过普通元数据列过滤。有效视图、原文导出、历史搜索、解压和响应链读取使用相同的正文解码规则。
- 首次后台转换前用原生 `VACUUM INTO` 生成一致备份，校验完整性、同步并以私有权限保留；转换分批事务提交，中断可重入。新库无需历史转换。不能用旧版直接读取已转为 BLOB 的正文；回滚保留备份，不自动删除用户备份。
- 修复已有 `user_version` 标记之后仍有长指纹的情况：后台按批转换为 SHA-256，而非仅相信版本号。此阶段未引入 `content` 表；正文去重属于第二阶段，不能把去重收益与 zstd 收益相加。
- 每连接设置 WAL 保留目标 8 MiB、自动 checkpoint 256 页；后台记录 PASSIVE checkpoint 的 busy/log/checkpointed 结果。日志已全部 checkpoint 且达到 2048 页时，零等待尝试 TRUNCATE；关闭连接也零等待尝试截断。长读事务会阻止回收，目标不是硬上限，绝不手工删除 WAL。空闲页用增量 vacuum 回收；启动路径不做全库 VACUUM。
- CLI：`codex-mixin --no-tui context maintain --batches 1000 --json` 执行有界维护并返回待处理状态、原始/存储字节和 WAL 结果。显式加 `--compact` 才执行全库 VACUUM；它不删除历史。数据库迁移后仍会随新历史增长。

2026-11-06 故障副本验证（253 个会话；非运行中的用户库）：

| 项目 | 转换前 | 转换后 |
| --- | --- | --- |
| 消息正文 UTF-8 字节 | 228,178,013 | 124,323,989（存储字节，减少 45.5%） |
| 回复正文 UTF-8 字节 | 2,084,552 | 1,324,181（存储字节） |
| 主库文件 | 327,688,192 | 197,357,568（显式压实后，减少 39.8%） |
| WAL 文件 | — | 0 |
| 长指纹 | 7,576 | 0 |
| 消息 / 回复 / 历史引用 / 摘要 | 89,819 / 17,300 / 66,599 / 740 | 全部不变 |
| 旧版本遗留缺失消息 / 回复 | 33,996 / 16,995 | 全部不变，未新增丢失 |

转换、校验和显式压实耗时 26.79 秒（debug CLI，包含首次备份）。通过独立 Node zstd 解码逐条核对 55,823 条可用消息和 305 条可用回复，验证原文长度和 SHA-256；全部消息及回复的顺序聚合哈希与转换前完全一致。会话、segments、turns 数量亦未变；`integrity_check=ok`。备份体积不计入主库优化比例。

既有 HTTP/CLI E2E 扩展覆盖旧 TEXT 与新 BLOB 混读、闲置及无引用历史保留、一致备份、读事务阻塞 WAL 回收、完整导出、搜索、实际解压和再次压缩、校验损坏明确失败、响应链续聊及全历史分叉修复。默认并行 Rust 集成测试 253 通过、7 项既有忽略；默认及 client-auth 严格 Clippy、架构检查、client-auth 4 项、multi-provider 和 prompt-cache 32 项通过。所有探针使用隔离环境；没有把工作线程数设为产品并发上限，也未迁移真实用户库或替换运行中的 App/网关。

## 原文过期后的统计与续聊回归

- 原因：预览和精简导出对空正文调用 `json_extract`，统计又直接渲染过期原文，使三个客户端共用的 CLI 查询失败。修复后导出显式标记过期记录，统计保留消息数量并通过 `expired_messages` 标记不可用原文；token 估算不含这些原文。解压过期原文仍明确失败。
- 全历史重放在压缩块中分叉时，以前会暴露已过期的前缀，未恢复正文。现在只从指纹匹配的输入恢复实际可见的原文，分叉、恢复和追加在同一事务提交；未过期原文不重复序列化，仍有摘要覆盖的原文不恢复。
- 既有 HTTP/CLI E2E 覆盖过期后的精简导出、响应链续聊、压缩块内分叉、历史遗留缺失正文的统计，以及完整重放恢复。原始故障副本中的 CLI 查询也用于验证。
- 磁盘证据：598,421,504 字节的副本含约 170 MB 空闲页，以及约 125 MB 位于仍使用页面内部的空洞。增量回收只能回收空闲页。副本执行原生 `VACUUM` 后为 327,688,192 字节，用时 2.81 秒，89,819 条消息、253 个会话、740 个摘要块和 228,178,013 字节消息正文均保留，完整性检查为 `ok`。此结果来自副本，不表示已压实用户原库；数据库继续保留当前原文、摘要和引用，不会归零。
