<!-- codex-mixin:zh-Hans:start -->
## v0.4.0

这是一次覆盖 macOS 体验、Provider 可观测性、百度原生认证、图片与长会话稳定性的完整升级。

### 全新的 Provider 使用中心

- 重新设计 macOS 菜单栏的 Provider 使用面板，将额度、Token 使用和缓存命中集中到统一界面
- 按 Provider 分组展示多个额度来源，使用紧凑的品牌卡片和 Provider 图标，减少多 Provider 场景下的菜单长度
- 按模型拆分输入、输出、缓存读取和缓存写入 Token，支持交互式使用量条和详细悬浮信息
- 将 Token 与缓存使用历史持久化到 SQLite，网关和 App 重启后仍可查看累计数据
- 额度查询改为并发执行，Token 历史会优先展示，不再等待较慢的 Provider 额度接口
- 支持从 Provider 官网加载图标，并为百度、DeepSeek、OpenAI、OpenCode、OpenRouter 和自定义 Provider 提供内置图标
- 优化 Provider 设置窗口布局、刷新状态和异常状态展示，已配置但暂时不可用的 Provider 不再从界面消失

### 百度 OneAPI 原生认证升级

- 新增托管 DUCX 认证核心，并可在 macOS Provider 设置中选择、安装和完成二维码登录
- 将 DUCC 和 DUCX 统一为轻量级原生 Header 产生器，移除长期驻留的高内存认证 worker
- 认证核心只运行短命 warmup 回合，抓取 `comate_custom_header`、`Authorization` 等原生 Header 后立即停止，不会把 warmup 请求发送到真实 OneAPI
- 真实模型、请求体、工具和上下文始终由 Codex Mixin 控制，DUCC/DUCX 只提供原生认证 Header
- 按 Provider 隔离认证 runtime，多个百度 Provider 不会复用错误的账号、密钥或可执行文件
- 修复认证 Header 捕获代理在刷新后持续占用 task 和监听端口的问题
- Provider 探测、Web Search 探测和模型请求统一使用完整的百度原生认证 Header，并在认证桥被禁用时严格遵守配置
- 增加百度代码使用上报，通过托管 `data-report` 接入 Codex hooks，并仅上报明确启用了上报功能的百度模型
- 为 `hooks.json` 更新增加文件锁和原子替换，避免并发配置写入造成数据丢失或文件损坏
- 严格校验托管 DUCX 登录身份，多账号状态下不再选择不确定的上报账号

### 图片与长会话稳定性

- HTTP 和 WebSocket 解压后 JSON 请求统一支持最大 256 MiB，满足大图、长上下文和复杂工具历史场景，同时防止无界输入耗尽内存或磁盘
- 工具截图只在模型尚未看过的当前轮次中携带，历史轮次使用固定占位符，避免 base64 图片随长会话持续累积
- 新鲜截图会压缩到视觉模型预算内；上游返回 413 时，将内嵌图片进一步压缩并只重试一次
- 修复 Responses Provider 路径重复压缩同一图片的问题，避免额外 CPU 消耗和二次有损 JPEG 编码
- 同时支持 Responses、Anthropic Messages 和 Chat Completions 的图片归一化与 413 重试
- 保持 prompt cache 前缀稳定，图片处理不会破坏系统提示、工具定义和已复用历史的字节一致性
- 无法解析的工具截图会替换为稳定标记，单张损坏截图不再破坏整个长会话
- 上游只有 413 会保留原状态以触发图片回退；其他上游错误统一返回脱敏的 502，避免暴露 Provider 内部响应和拓扑信息

### 安装与托管能力

- 安装到 Codex 前自动查找可用的 Codex CLI，包括环境变量、ChatGPT/Codex App 内置 CLI、`~/.local/bin` 和 `PATH`
- 本机缺少 Codex CLI 时，可自动运行官方安装流程，将独立 CLI 安装到 `~/.local/bin`
- 网关启动不再等待官方模型目录网络请求；启动完成后在后台刷新目录，减少启动阻塞
- 增加托管 Skill 守护机制，Codex Desktop 更新覆盖系统 Skill 后可自动恢复 Codex Mixin 管理的 Skill
- 修复托管 `imagegen` Skill 的路径、默认模型和模型固定逻辑，避免工作流使用错误模型
- 修复官方 Realtime 和 Live Voice 路由，配置了自定义 Provider 后仍会保留可用的官方语音能力
- 修复自定义 Provider 协议探测顺序，保持 Responses、Messages 和 Chat Completions 的能力优先级
- 加强 Provider 和 Web Search 能力探测，只有完整合法的响应才会被记录为支持

### 其他改进

- 压缩网关日志格式，减少长时间运行时的日志体积
- 修复 macOS 后台命令输出未及时消费可能导致的进程阻塞
- 修复 Provider 名称、图标、额度来源和未知 Provider 的多个显示边界问题
- 修复使用量数据库初始化失败未反馈到用户的问题
- 补充菜单布局、Provider 设置、额度解析、图片处理、prompt cache、认证隔离和网关启动测试
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.4.0

這是一次涵蓋 macOS 體驗、Provider 可觀測性、百度原生認證、圖片與長執行緒穩定性的完整升級。

### 全新的 Provider 使用中心

- 重新設計 macOS 選單列的 Provider 使用面板，將額度、Token 使用和快取命中集中到統一介面
- 按 Provider 分組顯示多個額度來源，使用緊湊的品牌卡片和 Provider 圖示，減少多 Provider 場景下的選單長度
- 按模型拆分輸入、輸出、快取讀取和快取寫入 Token，支援互動式使用量條和詳細懸浮資訊
- 將 Token 與快取使用歷史持久化到 SQLite，Gateway 和 App 重新啟動後仍可查看累計資料
- 額度查詢改為並行執行，Token 歷史會優先顯示，不再等待較慢的 Provider 額度端點
- 支援從 Provider 官網載入圖示，並為百度、DeepSeek、OpenAI、OpenCode、OpenRouter 和自訂 Provider 提供內建圖示
- 最佳化 Provider 設定視窗版面、更新狀態和異常狀態顯示，已設定但暫時不可用的 Provider 不再從介面消失

### 百度 OneAPI 原生認證升級

- 新增受管 DUCX 認證核心，並可在 macOS Provider 設定中選擇、安裝和完成 QR Code 登入
- 將 DUCC 和 DUCX 統一為輕量級原生 Header 產生器，移除長期駐留的高記憶體認證 worker
- 認證核心只執行短命 warmup 回合，擷取 `comate_custom_header`、`Authorization` 等原生 Header 後立即停止，不會將 warmup 請求傳送到真實 OneAPI
- 真實模型、請求內容、工具和上下文始終由 Codex Mixin 控制，DUCC/DUCX 只提供原生認證 Header
- 按 Provider 隔離認證 runtime，多個百度 Provider 不會重用錯誤的帳號、金鑰或執行檔
- 修正認證 Header 擷取代理在更新後持續佔用 task 和監聽連接埠的問題
- Provider 探測、Web Search 探測和模型請求統一使用完整的百度原生認證 Header，並在認證橋停用時嚴格遵守設定
- 增加百度程式碼使用回報，透過受管 `data-report` 接入 Codex hooks，並僅回報明確啟用了回報功能的百度模型
- 為 `hooks.json` 更新增加檔案鎖和原子替換，避免並行設定寫入造成資料遺失或檔案損壞
- 嚴格驗證受管 DUCX 登入身分，多帳號狀態下不再選擇不確定的回報帳號

### 圖片與長執行緒穩定性

- HTTP 和 WebSocket 解壓後 JSON 請求統一支援最大 256 MiB，滿足大圖、長上下文和複雜工具歷史場景，同時防止無界輸入耗盡記憶體或磁碟
- 工具截圖只在模型尚未查看的目前輪次中攜帶，歷史輪次使用固定佔位符，避免 base64 圖片隨長執行緒持續累積
- 新鮮截圖會壓縮到視覺模型預算內；上游回傳 413 時，將內嵌圖片進一步壓縮並只重試一次
- 修正 Responses Provider 路徑重複壓縮同一圖片的問題，避免額外 CPU 消耗和二次有損 JPEG 編碼
- 同時支援 Responses、Anthropic Messages 和 Chat Completions 的圖片正規化與 413 重試
- 保持 prompt cache 前綴穩定，圖片處理不會破壞系統提示、工具定義和已重用歷史的位元組一致性
- 無法解析的工具截圖會替換為穩定標記，單張損壞截圖不再破壞整個長執行緒
- 上游只有 413 會保留原狀態以觸發圖片回退；其他上游錯誤統一回傳脫敏的 502，避免暴露 Provider 內部回應和拓撲資訊

### 安裝與受管能力

- 安裝到 Codex 前自動尋找可用的 Codex CLI，包括環境變數、ChatGPT/Codex App 內建 CLI、`~/.local/bin` 和 `PATH`
- 本機缺少 Codex CLI 時，可自動執行官方安裝流程，將獨立 CLI 安裝到 `~/.local/bin`
- Gateway 啟動不再等待官方模型目錄網路請求；啟動完成後在背景更新目錄，減少啟動阻塞
- 增加受管 Skill 守護機制，Codex Desktop 更新覆蓋系統 Skill 後可自動還原 Codex Mixin 管理的 Skill
- 修正受管 `imagegen` Skill 的路徑、預設模型和模型固定邏輯，避免工作流程使用錯誤模型
- 修正官方 Realtime 和 Live Voice 路由，設定了自訂 Provider 後仍會保留可用的官方語音能力
- 修正自訂 Provider 協議探測順序，保持 Responses、Messages 和 Chat Completions 的能力優先順序
- 加強 Provider 和 Web Search 能力探測，只有完整合法的回應才會被記錄為支援

### 其他改進

- 壓縮 Gateway 日誌格式，減少長時間執行時的日誌體積
- 修正 macOS 背景命令輸出未及時讀取消費可能導致的程序阻塞
- 修正 Provider 名稱、圖示、額度來源和未知 Provider 的多個顯示邊界問題
- 修正使用量資料庫初始化失敗未回報給使用者的問題
- 補充選單版面、Provider 設定、額度解析、圖片處理、prompt cache、認證隔離和 Gateway 啟動測試
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.4.0

This release is a substantial upgrade to the macOS experience, provider observability, native Baidu authentication, image handling, and long-thread reliability.

### New Provider Usage Center

- Redesign the macOS menu bar provider dashboard to combine quota, token usage, and cache performance in one interface
- Group multiple quota sources by provider and use compact branded cards to keep multi-provider menus manageable
- Break down input, output, cache-read, and cache-write tokens by model, with interactive usage bars and detailed tooltips
- Persist token and cache history in SQLite so cumulative usage remains available after gateway and app restarts
- Query provider quotas concurrently and show local token history without waiting for slow quota endpoints
- Load icons from provider websites and include built-in assets for Baidu, DeepSeek, OpenAI, OpenCode, OpenRouter, and custom providers
- Stabilize provider settings, refresh states, and error presentation so configured providers remain visible when temporarily unavailable

### Native Baidu OneAPI Authentication

- Add a managed DUCX authentication core with installation and QR-code login from macOS provider settings
- Unify DUCC and DUCX as lightweight native-header generators, replacing the persistent high-memory authentication worker
- Run only a short authentication warmup, capture native headers such as `comate_custom_header` and `Authorization`, and stop before the request reaches the real OneAPI
- Keep the real model, request body, tools, and context under Codex Mixin control; DUCC and DUCX supply authentication headers only
- Isolate authentication runtimes by provider so multiple Baidu providers cannot share the wrong account, key, or executable
- Stop authentication capture proxies after refreshes so they do not leak tasks or listening ports
- Use complete native Baidu headers for provider discovery, Web Search probing, and model requests, while respecting explicitly disabled authentication bridges
- Add Baidu code-usage reporting through managed `data-report` Codex hooks, scoped to models whose provider explicitly enables reporting
- Lock and atomically replace `hooks.json` updates to prevent concurrent writes from losing configuration or corrupting the file
- Require exactly one managed DUCX login identity so usage reports cannot select an ambiguous account

### Images and Long-Thread Reliability

- Set a shared 256 MiB limit for decompressed HTTP and WebSocket JSON requests, supporting large images, long contexts, and complex tool history without allowing unbounded memory or disk use
- Carry a tool screenshot only until the model has seen it, then replay a stable marker so base64 image history does not grow indefinitely
- Compress fresh screenshots to the vision budget and retry one upstream 413 with a smaller image profile
- Prevent Responses provider routes from compressing the same image twice, avoiding redundant CPU work and repeated lossy JPEG encoding
- Apply image normalization and 413 retry handling consistently across Responses, Anthropic Messages, and Chat Completions
- Preserve prompt-cache prefix bytes across system prompts, tool definitions, and reusable history
- Replace unreadable tool screenshots with a stable marker so one damaged screenshot does not terminate a long thread
- Preserve upstream 413 responses for image fallback while converting other upstream failures to redacted 502 responses

### Installation and Managed Integrations

- Locate an installed Codex CLI through environment configuration, bundled ChatGPT/Codex app resources, `~/.local/bin`, or `PATH`
- Run the official non-interactive installer into `~/.local/bin` when no Codex CLI is available
- Start the gateway without waiting for the official model catalog network request, then refresh the catalog in the background
- Add a managed-skill guard that restores Codex Mixin skills after Codex Desktop updates replace system-managed files
- Fix the managed `imagegen` skill path, workflow default model, and pinned model behavior
- Preserve official Realtime and Live Voice routing when custom providers are configured
- Preserve Responses, Messages, and Chat Completions priority when detecting custom-provider protocols
- Tighten provider and Web Search capability probes so support is recorded only after a complete valid response

### Additional Improvements

- Compact gateway log formatting to reduce long-running log growth
- Drain verbose macOS subprocess output to prevent background processes from blocking
- Fix provider-name, icon, quota-source, and unknown-provider display edge cases
- Surface usage-database initialization failures instead of silently hiding usage history
- Expand tests for menu layout, provider settings, quota parsing, images, prompt caching, authentication isolation, and gateway startup
<!-- codex-mixin:en:end -->
