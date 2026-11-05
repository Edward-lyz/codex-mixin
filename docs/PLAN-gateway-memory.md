# PLAN：网关与菜单栏内存压缩

状态：已完成（分支 codex/perf-disconnect-openai）。

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

## 后续候选
- tokio blocking 线程池上限与线程栈。
- 菜单栏：仅在测量确认收益时改动，不改变外观。
