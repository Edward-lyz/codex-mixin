<!-- codex-mixin:zh-Hans:start -->
## v0.3.8

- Baidu OneAPI 改用 Codex Mixin 自管的 DUCX app-server；缺失时由用户确认下载，随后在独立终端扫码登录。
- 每条 Responses 上游子请求都先经过真实 DUCX turn，再由本机一次性桥接请求转发。网关不提取、保存、重建或伪造 DUCX 认证 Header，也不允许重放桥接请求。
- DUCX 进程按需启动并持续复用；统一执行层覆盖普通 HTTP/WS、Fusion 及其高级功能子请求。桥接层会移除 DUCX 注入内容，并阻止 hooks 或配置污染。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.8

- Baidu OneAPI 改用 Codex Mixin 自管的 DUCX app-server；缺少時由使用者確認下載，之後在獨立終端掃碼登入。
- 每條 Responses 上游子請求都先經過真實 DUCX turn，再由本機一次性橋接請求轉送。Gateway 不擷取、儲存、重建或偽造 DUCX 認證 Header，也不允許重放橋接請求。
- DUCX 行程按需啟動並持續重用；統一執行層涵蓋一般 HTTP/WS、Fusion 及其進階功能子請求。橋接層會移除 DUCX 注入內容，並阻止 hooks 或設定污染。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.8

- Baidu OneAPI now uses a Codex Mixin-managed DUCX app-server. When it is missing, the user confirms the download and then completes QR-code login in a dedicated Terminal window.
- Every upstream Responses subrequest creates a real DUCX turn and is then relayed through a single-use local bridge. The gateway does not extract, store, reconstruct, or forge DUCX authentication headers, and bridge requests cannot be replayed.
- The DUCX process starts lazily and remains reusable. The unified executor covers HTTP/WS, Fusion, and advanced-feature subrequests; the bridge removes DUCX-injected content and fails closed on hook or configuration contamination.
<!-- codex-mixin:en:end -->
