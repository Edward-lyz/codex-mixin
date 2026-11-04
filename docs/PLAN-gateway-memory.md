# PLAN：网关与菜单栏内存压缩

状态：实施中（分支 codex/perf-disconnect-openai）。

## 目标
- 网关 `codex-mixin start` 常驻 phys_footprint < 10 MB；菜单栏 UI < 50 MB。
- 展示与交互不变；每项改动附测量数据。

## 第一步：按需查询每轮用量（src/gateway/turn_usage.rs）
- 现状：`TurnLedger::load` 启动时把最近 5000 行读进 `VecDeque<TurnUsage>`，每行多个 String，
  常驻约 3–4 MB，`record` 再 clone 一份进内存。`/v1/usage/turns` 已在 blocking 线程里调用 `query`。
- 改后：内存里不再保存行；`query` 直接在 usage.sqlite3 上按 `at_ms DESC, id DESC` 过滤 since/provider，
  逐行读取直到 limit；共用已有的写连接并把页缓存限制在 256 KB。
- 风险：刚完成的一轮在异步落盘前查询不到（毫秒级）；数据库不可读时查询返回空并记 warning。

## 后续候选（逐项测量后决定）
- models.dev 目录缓存（src/provider/resolver.rs `MODELS_DEV_CATALOG_CACHE`）在发现完成后不再常驻。
- tokio blocking 线程池上限与线程栈。
- 菜单栏：仅在测量确认收益时改动，不改变外观。
