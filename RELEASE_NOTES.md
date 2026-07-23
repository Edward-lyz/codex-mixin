<!-- codex-mixin:zh-Hans:start -->
## v0.3.3

- 官方模型目录现在会在 Gateway 启动时在线同步，并每 5 分钟自动刷新；手动刷新命令也使用相同来源，不再被旧的本地 `models_cache.json` 覆盖。
- 在线同步失败时保留当前可用目录；仅首次离线安装回退到本地缓存，避免已更新的上下文窗口回滚。
- GPT 自定义上游别名的上下文窗口会与官方同名模型取较小值。官方目录为 272K 时，对应的 `sol`、`terra`、`luna` provider 别名也会同步为 272K。
- 修复自定义模型完成多轮工具调用后，Codex App 无法自动折叠执行过程的问题。中间工具轮次现在标记为 `commentary`，最终回答标记为 `final_answer`。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.3

- 官方模型目錄現在會在 Gateway 啟動時線上同步，並每 5 分鐘自動更新；手動更新命令也使用相同來源，不再被舊的本機 `models_cache.json` 覆蓋。
- 線上同步失敗時保留目前可用目錄；僅首次離線安裝回退到本機快取，避免已更新的上下文視窗回滾。
- GPT 自訂上游別名的上下文視窗會與官方同名模型取較小值。官方目錄為 272K 時，對應的 `sol`、`terra`、`luna` provider 別名也會同步為 272K。
- 修復自訂模型完成多輪工具呼叫後，Codex App 無法自動摺疊執行過程的問題。中間工具輪次現在標記為 `commentary`，最終回答標記為 `final_answer`。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.3

- The official model catalog is now synchronized online when the Gateway starts and refreshed every five minutes. Manual refresh uses the same source and can no longer be overwritten by a stale local `models_cache.json`.
- A failed online sync preserves the current usable catalog. Only a first-time offline installation falls back to the local cache, preventing updated context windows from being rolled back.
- GPT custom-upstream aliases now clamp their context window to the matching official model. When the official catalog reports 272K, the corresponding `sol`, `terra`, and `luna` provider aliases also use 272K.
- Fixed Codex App failing to collapse the execution history after custom models complete multi-turn tool calls. Intermediate tool turns are now marked as `commentary`, while the terminal response is marked as `final_answer`.
<!-- codex-mixin:en:end -->
