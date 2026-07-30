<!-- codex-mixin:zh-Hans:start -->
## v0.3.8

- Baidu OneAPI 新增可选的 DUCC Header 认证，默认关闭。首次配置和旧配置未作选择时，macOS 会显示明确的风险确认；只有用户主动启用后才会要求本机已安装、登录 Comate 和 DUCC。
- 启用后，所有 Baidu OneAPI 推理路径（Responses、Messages、图片、Realtime、测速和 Web Search 探测）都会携带本机 DUCC 签发的 `comate_custom_header`。签发失败时请求会被阻止，不会静默回退到普通计费请求。
- 网关同时复用 Comate 的 `data-report`，按 DUCC 的 SessionStart、UserPromptSubmit、Stop、SessionEnd 生命周期上报请求使用信息。上报失败只记录警告，不中断模型响应。DUCC Header 不保证免计费；相关费用、数据、账号和合规风险由用户自行承担（适用法律另有规定除外）。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.8

- Baidu OneAPI 新增可選的 DUCC Header 認證，預設關閉。首次設定和舊設定尚未選擇時，macOS 會顯示明確的風險確認；只有使用者主動啟用後，才會要求本機已安裝並登入 Comate 和 DUCC。
- 啟用後，所有 Baidu OneAPI 推理路徑（Responses、Messages、圖片、Realtime、測速和 Web Search 探測）都會攜帶本機 DUCC 簽發的 `comate_custom_header`。簽發失敗時會阻止請求，不會靜默回退至一般計費請求。
- Gateway 同時重用 Comate 的 `data-report`，依 DUCC 的 SessionStart、UserPromptSubmit、Stop、SessionEnd 生命週期上報請求使用資訊。上報失敗只記錄警告，不中斷模型回應。DUCC Header 不保證免計費；相關費用、資料、帳號和合規風險由使用者自行承擔（適用法律另有規定除外）。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.8

- Added optional DUCC Header authentication for Baidu OneAPI, off by default. macOS shows an explicit risk acknowledgment during first-time setup and for legacy configurations with no recorded choice. Comate and DUCC are required only after the user opts in.
- Once enabled, every Baidu OneAPI inference path—Responses, Messages, images, Realtime, benchmarks, and Web Search probes—carries a locally signed `comate_custom_header`. Signing failures block the request instead of silently falling back to ordinary billing.
- The gateway also reuses Comate's `data-report` program to report request-usage information through DUCC's SessionStart, UserPromptSubmit, Stop, and SessionEnd lifecycle. Reporting failures are logged without interrupting model responses. A DUCC Header does not guarantee billing exemption; users accept the resulting billing, data, account, and compliance risks, except where applicable law provides otherwise.
<!-- codex-mixin:en:end -->
