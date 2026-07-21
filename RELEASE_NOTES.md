<!-- codex-mixin:zh-Hans:start -->
## v0.3.1

- 修复启动时自动发现新版本后，在后台 cooperative queue 创建更新弹窗导致 App `SIGABRT` 闪退的问题。
- 更新检查与更新提示现在强制运行在 MainActor；网络请求完成后也会安全返回主线程再操作 AppKit。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.1

- 修正啟動時自動發現新版本後，在背景 cooperative queue 建立更新提示而造成 App `SIGABRT` 閃退的問題。
- 更新檢查與更新提示現在強制在 MainActor 執行；網路請求完成後也會安全返回主執行緒再操作 AppKit。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.1

- Fixed a startup `SIGABRT` when the automatic update check found a newer release and created its AppKit prompt on a cooperative background queue.
- Update checks and prompts now stay on MainActor, including after the network request resumes.
<!-- codex-mixin:en:end -->
