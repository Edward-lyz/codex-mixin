<!-- codex-mixin:zh-Hans:start -->
## v0.3.5

- 新增 Codex Desktop 官方账号与 Custom-only 两种安装路径。Custom-only 可在没有官方登录时使用本地占位认证加载自定义模型，并在安装、切换和卸载时明确提示官方插件、云任务及账户功能的差异。
- 重做 Provider 设置窗口，缩小无效空白并修复表单横向不可达问题；Provider 选择器始终以本地配置为唯一数据源，不再因历史测速快照漏掉供应商。
- 将模型选择与测速合并为单 Provider 模型表，支持搜索、筛选、Codex allowlist、TTFT、吞吐、上下文和 Baidu OneAPI 倍率展示；所有列均可排序。
- 延迟测速默认使用 5 秒超时并在首 token 到达时立即完成，避免上游迟迟不关闭流造成假超时；完整模式继续提供吞吐测试。
- App 的“自动检测并修复”新增快速路径，使用缓存与网关实际状态，跳过无关的上游和 Codex 内核深度实测；热运行由约 8 秒降至约 0.05 秒，CLI 普通 `doctor` 仍保留完整检查。
- 加快模型目录与菜单状态刷新频率，并补充 Custom 安装、Desktop 模型选择、Provider 布局及 Doctor 性能回归测试。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.5

- 新增 Codex Desktop 官方帳號與 Custom-only 兩種安裝路徑。Custom-only 可在沒有官方登入時使用本機佔位認證載入自訂模型，並在安裝、切換和解除安裝時明確提示官方外掛、雲端任務及帳號功能的差異。
- 重做 Provider 設定視窗，縮小無效空白並修復表單橫向不可達問題；Provider 選擇器固定以本機設定為唯一資料來源，不再因歷史測速快照漏掉供應商。
- 將模型選擇與測速合併為單 Provider 模型表，支援搜尋、篩選、Codex allowlist、TTFT、吞吐、上下文和 Baidu OneAPI 倍率顯示；所有欄位均可排序。
- 延遲測速預設使用 5 秒逾時並在首個 token 到達時立即完成，避免上游遲遲不關閉串流造成假逾時；完整模式繼續提供吞吐測試。
- App 的「自動檢測並修復」新增快速路徑，使用快取與 Gateway 實際狀態，跳過無關的上游和 Codex 核心深度實測；熱執行由約 8 秒降至約 0.05 秒，CLI 普通 `doctor` 仍保留完整檢查。
- 加快模型目錄與選單狀態更新頻率，並補充 Custom 安裝、Desktop 模型選擇、Provider 版面配置及 Doctor 效能回歸測試。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.5

- Added separate official-account and Custom-only installation paths for Codex Desktop. Custom-only installations can load custom models without an official login through a managed local placeholder, with clear warnings about unavailable official plugins, cloud tasks, and account features.
- Redesigned Provider Settings to remove wasted space and unreachable horizontal content. Provider pickers now use local configuration as their sole data source, so benchmark history can no longer hide configured providers.
- Combined model selection and benchmarking into a single-provider model table with search, filters, the Codex allowlist, TTFT, throughput, context, and Baidu OneAPI rate metadata. Every column is sortable.
- Latency benchmarks now default to a five-second timeout and finish as soon as the first token arrives, preventing false timeouts when an upstream leaves its stream open. Full benchmarks remain available for throughput measurement.
- Added a fast path for the App's Automatic Diagnostics and Repair action. It uses cached/provider gateway state and skips unrelated upstream and Codex engine deep probes, reducing warm execution from roughly eight seconds to around 0.05 seconds. Regular CLI `doctor` retains the complete checks.
- Increased model-catalog and menu refresh frequency and added regression coverage for Custom installation, Desktop model selection, Provider layout, and Doctor performance.
<!-- codex-mixin:en:end -->
