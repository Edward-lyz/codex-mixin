<!-- codex-mixin:zh-Hans:start -->
## v0.4.2

本版本聚焦于更新体验、跨客户端安装和数据上报 hook。

### 更新与安装

- macOS App 更新现在会自动下载、挂载 DMG，并替换当前 App，不再要求用户手动拖入 Applications
- 替换前会优雅停止本地网关，替换完成后自动重新启动 App 和网关
- 使用 staging bundle 完成替换，避免旧版本残留文件混入新版本
- 安装到 Claude Code 或 DSH 时，如果启用了百度代码使用上报，会自动安装对应客户端的全局 reporting hook
- 卸载 Claude Code 或 DSH 时会清理 Codex Mixin 管理的 hook，同时保留用户自己的配置

### 平台与能力

- 增加 DSH（DeepSeek Harness）provider 集成
- 增加 Fusion provider 的禁用和删除命令
- 修复 DSH Responses SSE relay 的 malformed event 处理
- 改进 gateway 配置更新、provider readiness 和 native reporting 生命周期

### 验证

- `cargo fmt --all -- --check`
- `cargo check --locked`
- report hook 单元测试
- macOS App 构建、签名和 bundle 校验
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.4.2

本版本聚焦於更新體驗、跨客戶端安裝和資料上報 hook。

### 更新與安裝

- macOS App 更新現在會自動下載、掛載 DMG 並替換目前 App，不再要求使用者手動拖入 Applications
- 替換前會優雅停止本機閘道，替換完成後自動重新啟動 App 和閘道
- 使用 staging bundle 完成替換，避免舊版本殘留檔案混入新版本
- 安裝到 Claude Code 或 DSH 時，如果啟用百度程式碼使用上報，會自動安裝對應客戶端的全域 reporting hook
- 卸載 Claude Code 或 DSH 時會清理 Codex Mixin 管理的 hook，同時保留使用者自己的設定

### 平台與能力

- 增加 DSH（DeepSeek Harness）provider 整合
- 增加 Fusion provider 的停用和刪除命令
- 修正 DSH Responses SSE relay 的 malformed event 處理
- 改進 gateway 設定更新、provider readiness 和 native reporting 生命週期

### 驗證

- `cargo fmt --all -- --check`
- `cargo check --locked`
- report hook 單元測試
- macOS App 建置、簽名和 bundle 驗證
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.4.2

This release focuses on seamless updates, cross-client installation, and reporting hooks.

### Updates and installation

- macOS app updates now download, mount, and replace the current app without requiring a manual drag into Applications
- The running gateway is stopped gracefully before replacement, then the app and gateway restart automatically
- Replacement uses a staging bundle so stale files from the previous app are not merged into the new version
- Installing into Claude Code or DSH now installs a client-specific global reporting hook when Baidu code-usage reporting is enabled
- Uninstalling Claude Code or DSH removes only Codex Mixin managed hooks and preserves user configuration

### Platform and capabilities

- Add DSH (DeepSeek Harness) provider integration
- Add Fusion provider disable and delete commands
- Fix malformed Responses SSE relay events for DSH
- Improve gateway configuration updates, provider readiness, and native reporting lifecycle handling

### Validation

- `cargo fmt --all -- --check`
- `cargo check --locked`
- report hook unit tests
- macOS app build, signing, and bundle verification
<!-- codex-mixin:en:end -->

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
