<!-- codex-mixin:zh-Hans:start -->
## v0.3.10

### Claude Code 接入与全新 Mixin 卡片

- **支持 Claude Code：** 网关新增 Anthropic Messages 兼容的 `/v1/messages` 端点；新增 `install-claude`、`uninstall-claude` 和 `claude-status`，菜单栏 App 可直接安装或恢复 Claude Code 配置。
- **全新关于页：** 新增原生 macOS 关于窗口，集中显示版本、Build 和 GitHub 仓库入口，并支持一键复制完整版本信息。
- **个性化 Mixin 卡片：** 关于页左侧默认展示可互动纪念卡；卡片结合本机安装身份与使用天数，支持 hover、拖动、点击放大、保存 PNG 和系统分享。
- **NASA 月度 4K 壁纸：** 构建流程自动同步并打包当月 NASA ISS 壁纸。每次打开关于页会选择一张与上次不同的背景，窗口打开期间保持不变。
- **修复老用户天数：** 首次生成身份时会从 `~/.codex-mixin` 的历史创建时间做 best-effort 回迁，避免升级后错误显示为使用第 1 天。
- macOS App 最低系统版本调整为 macOS 13.1。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.10

### Claude Code 接入與全新 Mixin 卡片

- **支援 Claude Code：** Gateway 新增 Anthropic Messages 相容的 `/v1/messages` 端點；新增 `install-claude`、`uninstall-claude` 與 `claude-status`，選單列 App 可直接安裝或還原 Claude Code 設定。
- **全新關於頁：** 新增原生 macOS 關於視窗，集中顯示版本、Build 與 GitHub 儲存庫入口，並支援一鍵複製完整版本資訊。
- **個人化 Mixin 卡片：** 關於頁左側預設顯示可互動紀念卡；卡片結合本機安裝身分與使用天數，支援 hover、拖曳、點擊放大、儲存 PNG 與系統分享。
- **NASA 每月 4K 桌布：** 建置流程自動同步並封裝當月 NASA ISS 桌布。每次開啟關於頁會選擇一張與上次不同的背景，視窗開啟期間保持不變。
- **修正舊使用者天數：** 首次產生身分時會從 `~/.codex-mixin` 的歷史建立時間做 best-effort 回遷，避免升級後錯誤顯示為使用第 1 天。
- macOS App 最低系統版本調整為 macOS 13.1。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.10

### Claude Code integration and the new Mixin card

- **Claude Code support:** The gateway now exposes an Anthropic Messages-compatible `/v1/messages` endpoint. New `install-claude`, `uninstall-claude`, and `claude-status` commands—and matching menu bar actions—install or restore Claude Code configuration.
- **A new About window:** The native macOS About window shows the version, build, and GitHub repository entry point, with one-click copying of complete version information.
- **A personalized Mixin card:** An interactive keepsake card now appears by default on the left side of the About window. It combines the local installation identity with usage history and supports hover, drag, click-to-enlarge, PNG export, and system sharing.
- **Monthly NASA 4K wallpapers:** The build pipeline automatically syncs and bundles the current NASA ISS wallpaper set. Each About-window opening selects a background different from the previous one and keeps it fixed while the window remains open.
- **Correct history for existing users:** Initial identity creation now performs a best-effort migration from historical creation dates under `~/.codex-mixin`, avoiding an incorrect “day 1” after upgrading.
- The minimum macOS version is now macOS 13.1.
<!-- codex-mixin:en:end -->
