<!-- codex-mixin:zh-Hans:start -->
## v0.3.9

### DUCC 高性能纯净桥接

- **显著降低延迟：** 网关启动后在后台预热单一认证载体，不阻塞监听；GLM、Claude、Opus 可直接复用，切换模型无需重启 DUCX。
- **解除并发阻塞：** 捕获认证后立即释放内部回合，真实上游请求并行继续，不再让并发请求串行等待首个 token。
- **杜绝无关模型调用：** 关闭 DUCX 非必要流量，提示建议和辅助请求不会再触发其他模型；一次性 request ID 白名单同时阻止辅助请求和重放请求离开本机。
- **强化认证隔离：** 每条 Responses 上游子请求都通过真实 DUCX turn 和一次性本机桥接转发；网关不提取、保存、重建或伪造 DUCX 认证 Header。
- **统一执行路径：** Codex Mixin 自管并持续复用 DUCX app-server，覆盖 HTTP、WebSocket、Fusion 及高级功能子请求；桥接层会移除 DUCX 注入内容，并在 hooks 或配置污染时安全终止。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.9

### DUCC 高效能純淨橋接

- **顯著降低延遲：** Gateway 啟動後在背景預熱單一認證載體，不阻塞監聽；GLM、Claude、Opus 可直接重用，切換模型無需重新啟動 DUCX。
- **解除並行阻塞：** 擷取認證後立即釋放內部回合，真實上游請求可並行繼續，不再讓並行請求依序等待第一個 token。
- **杜絕無關模型呼叫：** 關閉 DUCX 非必要流量，提示建議與輔助請求不再觸發其他模型；一次性 request ID 白名單同時阻止輔助與重放請求離開本機。
- **強化認證隔離：** 每條 Responses 上游子請求皆透過真實 DUCX turn 與一次性本機橋接轉送；Gateway 不擷取、儲存、重建或偽造 DUCX 認證 Header。
- **統一執行路徑：** Codex Mixin 自行管理並持續重用 DUCX app-server，涵蓋 HTTP、WebSocket、Fusion 與進階功能子請求；橋接層會移除 DUCX 注入內容，並在 hooks 或設定污染時安全終止。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.9

### High-performance, clean DUCC bridge

- **Much lower latency:** The gateway prewarms one authentication carrier in the background without delaying the listener. GLM, Claude, and Opus reuse it directly, with no DUCX restart when switching models.
- **No concurrency bottleneck:** The internal turn is released as soon as authentication is captured, allowing real upstream requests to continue concurrently instead of serializing on time to first token.
- **No unrelated model calls:** DUCX nonessential traffic is disabled, so prompt suggestions and helper requests cannot invoke other models. A single-use request-ID allowlist also keeps helper and replay requests local.
- **Stronger authentication isolation:** Every upstream Responses subrequest passes through a real DUCX turn and a single-use local bridge. The gateway never extracts, stores, reconstructs, or forges DUCX authentication headers.
- **One execution path:** Codex Mixin manages and reuses the DUCX app-server across HTTP, WebSocket, Fusion, and advanced-feature subrequests. The bridge removes DUCX-injected content and fails closed on hook or configuration contamination.
<!-- codex-mixin:en:end -->
