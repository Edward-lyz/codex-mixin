<!-- codex-mixin:zh-Hans:start -->
## v0.4.1

这是一个 DUCX 原生认证紧急修复版本。建议所有使用百度 OneAPI 认证桥的 v0.4.0 用户立即升级。

### 紧急修复

- 修复 OpenAI Responses 和 Chat Completions 路径未注入 DUCX 原生认证 Header，导致部分模型出现认证丢失、502 或无法请求的问题
- 统一 Anthropic Messages、OpenAI Responses 和 Chat Completions 的 DUCX 原生认证 Header 选择逻辑
- 修复切换 DUCX 后认证类型与可执行文件路径可能错配，造成选择 DUCX 却启动 DUCX 的问题
- 对认证类型和托管可执行文件布局增加强校验，错误配置现在会直接拒绝，不再带着错误认证核心运行
- 修复 DUCX 只凭登录文件存在就判定已登录的问题；现在会校验实际 `Bearer-<JWT>` 登录记录，损坏、空白或歧义记录会要求重新扫码
- 修复 macOS Provider 设置中未保存的认证选择看起来已经生效、但模型刷新仍使用旧配置的问题
- 修复 Provider 已恢复健康后，菜单栏仍长期显示 Provider 降级的问题；轻量健康检查现在会携带当前 Provider readiness

### 验证

- 已使用真实 DUCX 登录态完成隔离模型发现，成功刷新 23 个百度 OneAPI 模型
- Rust 单元测试、HTTP 集成测试、启动测试、WebSocket 测试和 macOS Swift 测试全部通过
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.4.1

這是 DUCX 原生認證的緊急修正版本。建議所有使用百度 OneAPI 認證橋的 v0.4.0 使用者立即升級。

### 緊急修正

- 修正 OpenAI Responses 和 Chat Completions 路徑未注入 DUCX 原生認證 Header，導致部分模型認證遺失、502 或無法請求
- 統一 Anthropic Messages、OpenAI Responses 和 Chat Completions 的 DUCX 原生認證 Header 選擇邏輯
- 修正切換 DUCX 後認證類型與執行檔路徑可能錯配，造成選擇 DUCX 卻啟動 DUCX
- 對認證類型和受管執行檔版面增加強制驗證，錯誤設定現在會直接拒絕
- 修正 DUCX 只憑登入檔案存在就判定已登入；現在會驗證實際 `Bearer-<JWT>` 登入記錄
- 修正 macOS Provider 設定中未儲存的認證選擇看似已生效，但模型更新仍使用舊設定
- 修正 Provider 恢復健康後，選單列仍長期顯示 Provider 降級

### 驗證

- 已使用真實 DUCX 登入狀態完成隔離模型探索，成功更新 23 個百度 OneAPI 模型
- Rust 單元、HTTP 整合、啟動、WebSocket 和 macOS Swift 測試全部通過
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.4.1

This is an emergency DUCX native-authentication fix. All v0.4.0 users of the Baidu OneAPI authentication bridge should upgrade immediately.

### Emergency fixes

- Fix missing DUCX native authentication headers on OpenAI Responses and Chat Completions routes, which caused authentication loss, 502 responses, or failed requests for some models
- Use one DUCX native-header selection path across Anthropic Messages, OpenAI Responses, and Chat Completions
- Fix authentication-core and executable-path mismatches that could start DUCX after DUCX was selected
- Reject authentication configurations whose managed executable layout does not match the selected DUCX or DUCX core
- Validate the actual DUCX `Bearer-<JWT>` login record instead of treating any login marker file as an authenticated session
- Prevent unsaved macOS authentication selections from appearing active while model discovery still uses the saved core
- Clear stale macOS Provider degradation status after the gateway reports that providers have recovered

### Validation

- Completed isolated model discovery with a real DUCX login and refreshed 23 Baidu OneAPI models
- Passed the Rust unit, HTTP integration, startup, WebSocket, and macOS Swift test suites
<!-- codex-mixin:en:end -->
