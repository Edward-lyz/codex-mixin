<!-- codex-mixin:zh-Hans:start -->
## v0.5.4

- 新增 Pi 集成，支持模型配置、thinking 和 DUCX 用量上报
- 修复自定义模型错误继承官方生命周期元数据的问题
- DUCX 认证过期时自动刷新并重试请求

<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.5.4

- 新增 Pi 整合，支援模型設定、thinking 和 DUCX 用量上報
- 修正自訂模型錯誤繼承官方生命週期中繼資料的問題
- DUCX 驗證過期時自動更新並重試請求

<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.5.4

- Add Pi integration with model configuration, thinking, and DUCX usage reporting
- Fix custom models inheriting official model lifecycle metadata
- Refresh expired DUCX authentication and retry the request

<!-- codex-mixin:en:end -->

<!-- codex-mixin:zh-Hans:start -->
## v0.5.3

本版本扩展 Claude Code 和 OpenCode 集成，补充 Amazon Bedrock 与 Anthropic 协议兼容，并加强百度 Provider 的访问控制、模型能力探测和 macOS 更新稳定性。

### 应用集成

- Claude Code 支持分别映射 Opus、Sonnet 和 Haiku 模型，可从 macOS、TUI 或 CLI 安装和恢复配置
- 新增 OpenCode 集成，在保留现有配置的同时接入已选模型、Fusion 模型和 reasoning variants
- OpenCode 支持安装 Mixin 管理的 prompt、代码编辑和 transcript 上报 hooks
- Codex、Claude Code、DSH 和 OpenCode 使用相互独立的 gateway client key；已有托管安装会自动迁移，卸载时撤销对应 key

### Provider 与模型

- gateway 可将 Anthropic Messages 请求、工具、图片、usage 和流式事件转换到 OpenAI 兼容 Provider
- 新增 Amazon Bedrock Mantle preset，支持使用 Bedrock API key 访问内置 Claude 模型
- ImageGen 可通过辅助 Provider 使用本地 gateway，并为已有百度配置补齐图片路由
- 模型能力探测新增 thinking 检测，保留 Provider 已声明但本次无法确定的能力状态
- 探测完成后自动刷新 Codex 模型目录；macOS 同时重载 gateway，使能力变化立即生效

### 官方模型与 macOS

- OpenAI 官方 Provider 支持刷新和排序模型；新发现的模型默认不选中，已被上游移除的模型会从选择中清理
- 官方模型选择会同步应用到 Codex、Claude Code 和 DSH，模型列表刷新最长等待 10 秒
- macOS 改用 Sparkle 检查和安装签名更新；自动安装停滞时可打开已验证的 DMG
- 修复 LaunchAgent 停止后立即重启时的 `launchctl bootstrap` 竞态

### 安全与稳定性

- 仅允许 Mixin 托管客户端使用 DUCX 提供的百度原生认证，复制 gateway 地址或通用凭据不能绕过客户端身份校验
- gateway client key 使用独立安全随机值，配置更新和安装失败时保持原子性并回滚未完成的凭据变更
- 修复 DUCX 启动前临时文件未关闭，以及认证管道提前关闭时的处理
- 增加协议转换、应用配置、能力探测、访问控制、macOS 更新和 gateway 重启相关回归测试

<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.5.3

本版本擴充 Claude Code 和 OpenCode 整合，補充 Amazon Bedrock 與 Anthropic 協定相容性，並加強百度 Provider 的存取控制、模型能力探測和 macOS 更新穩定性。

### 應用程式整合

- Claude Code 支援分別映射 Opus、Sonnet 和 Haiku 模型，可從 macOS、TUI 或 CLI 安裝和還原設定
- 新增 OpenCode 整合，在保留現有設定的同時接入已選模型、Fusion 模型和 reasoning variants
- OpenCode 支援安裝 Mixin 管理的 prompt、程式碼編輯和 transcript 上報 hooks
- Codex、Claude Code、DSH 和 OpenCode 使用彼此獨立的 gateway client key；既有託管安裝會自動遷移，解除安裝時撤銷對應 key

### Provider 與模型

- gateway 可將 Anthropic Messages 請求、工具、圖片、usage 和串流事件轉換到 OpenAI 相容 Provider
- 新增 Amazon Bedrock Mantle preset，支援使用 Bedrock API key 存取內建 Claude 模型
- ImageGen 可透過輔助 Provider 使用本機 gateway，並為既有百度設定補齊圖片路由
- 模型能力探測新增 thinking 檢測，保留 Provider 已宣告但本次無法確定的能力狀態
- 探測完成後自動更新 Codex 模型目錄；macOS 同時重新載入 gateway，使能力變更立即生效

### 官方模型與 macOS

- OpenAI 官方 Provider 支援更新和排序模型；新探索到的模型預設不選取，已被上游移除的模型會從選擇中清理
- 官方模型選擇會同步套用到 Codex、Claude Code 和 DSH，模型清單更新最長等待 10 秒
- macOS 改用 Sparkle 檢查和安裝簽名更新；自動安裝停滯時可開啟已驗證的 DMG
- 修正 LaunchAgent 停止後立即重新啟動時的 `launchctl bootstrap` 競態

### 安全與穩定性

- 僅允許 Mixin 託管客戶端使用 DUCX 提供的百度原生驗證，複製 gateway 位址或通用憑證無法繞過客戶端身分驗證
- gateway client key 使用獨立安全隨機值，設定更新和安裝失敗時保持原子性並回復未完成的憑證變更
- 修正 DUCX 啟動前暫存檔未關閉，以及驗證管道提前關閉時的處理
- 增加協定轉換、應用程式設定、能力探測、存取控制、macOS 更新和 gateway 重新啟動相關回歸測試

<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.5.3

This release expands Claude Code and OpenCode integration, adds Amazon Bedrock and Anthropic protocol compatibility, and strengthens Baidu provider access control, model capability probing, and macOS update reliability.

### Application integration

- Map Claude Code Opus, Sonnet, and Haiku model families independently, with installation and restoration through macOS, the TUI, or the CLI
- Add OpenCode integration while preserving existing configuration and exposing selected models, Fusion models, and reasoning variants
- Install Mixin-managed OpenCode hooks for prompt, code-edit, and transcript reporting
- Give Codex, Claude Code, DSH, and OpenCode independent gateway client keys, migrate existing managed installations automatically, and revoke each key on uninstall

### Providers and models

- Translate Anthropic Messages requests, tools, images, usage, and streaming events for OpenAI-compatible providers
- Add an Amazon Bedrock Mantle preset for accessing built-in Claude models with a Bedrock API key
- Route ImageGen through auxiliary providers over the local gateway and backfill image routes for existing Baidu configurations
- Probe thinking support and preserve provider-advertised capabilities when a probe is indeterminate
- Refresh the Codex model catalog after capability probing and reload the gateway on macOS so capability changes take effect immediately

### Official models and macOS

- Refresh and sort OpenAI official models, leave newly discovered models unselected, and prune models removed upstream
- Apply official model choices across Codex, Claude Code, and DSH, with a 10-second upper bound on model catalog refreshes
- Use Sparkle to check and install signed macOS updates, with a verified DMG fallback when automatic installation stalls
- Recover from the `launchctl bootstrap` race caused by restarting a LaunchAgent immediately after teardown

### Security and reliability

- Allow only Mixin-managed clients to use DUCX-backed native Baidu authentication, preventing copied gateway endpoints or generic credentials from bypassing client identity checks
- Generate independent cryptographically secure gateway client keys and roll back incomplete credential changes when configuration updates or installation fail
- Close the temporary file before starting DUCX and handle early authentication pipe closure
- Add regression coverage for protocol translation, application configuration, capability probing, access control, macOS updates, and gateway restarts

<!-- codex-mixin:en:end -->

<!-- codex-mixin:zh-Hans:start -->
## v0.5.2

本版本重点改进 DUCX 数据上报可靠性、macOS 操作窗口体验和用量展示，并提升自定义模型 compaction 稳定性。

### DUCX 数据上报

- 持久化保存失败的上报事件，网络或服务异常后可重试，避免事件丢失
- 支持扫描并重放本地历史 Session
- 即使部分事件上传失败，也保留已成功上传的结果，不重复处理
- 失败的 Session 会继续保留在队列中，并显示具体的服务端错误
- macOS 手动上报窗口展示加入队列、上传成功和保留重试的完整结果
- 兼容新版 DUCX 清理 proxy 环境变量后的认证 token 获取流程
- 仅对实际的 `apply_patch` 操作上报代码变更事件
- `report-replay --all-sessions --json` 支持结构化输出

### macOS

- 设置、安装、测速等窗口在切换焦点后继续保持显示，并可以通过 Command-Tab 切换
- 操作完成或失败后的结果保持显示，直到用户主动关闭
- 修复 Provider 设置、更新窗口和操作进度窗口的状态保持问题
- 限制用量面板最多显示三行 quota，更多 quota 通过滚动查看
- 修复多行 quota 与 token 范围选择器之间的布局间距

### 稳定性

- 忽略没有 provider 可见内容的 Codex compaction 边界标记
- 自定义模型遗漏 `submit_compaction` 时，自动重试同一个严格 compaction 请求
- 增加 DUCX 重放、macOS 窗口策略、安装进度和用量布局相关回归测试

<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.5.2

本版本重點改進 DUCX 資料上報可靠性、macOS 操作視窗體驗和用量顯示，並提升自訂模型 compaction 穩定性。

### DUCX 資料上報

- 持久化保存失敗的上報事件，網路或服務異常後可重試，避免事件遺失
- 支援掃描並重放本機歷史 Session
- 即使部分事件上傳失敗，也保留已成功上傳的結果，不重複處理
- 失敗的 Session 會繼續保留在佇列中，並顯示具體的伺服器錯誤
- macOS 手動上報視窗展示加入佇列、上傳成功和保留重試的完整結果
- 相容新版 DUCX 清理 proxy 環境變數後的驗證 token 取得流程
- 僅對實際的 `apply_patch` 操作上報程式碼變更事件
- `report-replay --all-sessions --json` 支援結構化輸出

### macOS

- 設定、安裝、測速等視窗在切換焦點後繼續保持顯示，並可以透過 Command-Tab 切換
- 操作完成或失敗後的結果保持顯示，直到使用者主動關閉
- 修正 Provider 設定、更新視窗和操作進度視窗的狀態保持問題
- 限制用量面板最多顯示三行 quota，更多 quota 透過捲動查看
- 修正多行 quota 與 token 範圍選擇器之間的版面間距

### 穩定性

- 忽略沒有 provider 可見內容的 Codex compaction 邊界標記
- 自訂模型遺漏 `submit_compaction` 時，自動重試同一個嚴格 compaction 請求
- 增加 DUCX 重放、macOS 視窗策略、安裝進度和用量版面相關回歸測試

<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.5.2

This release improves DUCX reporting reliability, macOS operation windows and quota display, and custom-model compaction stability.

### DUCX reporting

- Persist failed report events for retry after network or service failures
- Scan and replay local historical Sessions
- Preserve successful uploads when only part of a replay fails, without reprocessing them
- Keep failed Sessions queued and show the server error that caused the failure
- Show queued, delivered, and retained retry results in the macOS manual reporting window
- Support authentication token capture from DUCX builds that clear proxy environment variables
- Report code-change events only for actual `apply_patch` operations
- Add structured output to `report-replay --all-sessions --json`

### macOS

- Keep settings, installation, and benchmark windows visible when focus changes, and expose them through Command-Tab
- Keep completed and failed operation results visible until the user closes them
- Fix state preservation for provider settings, updater, and operation progress windows
- Show at most three quota rows in the usage panel and scroll additional rows
- Fix spacing between multiple quota rows and the token range picker

### Reliability

- Ignore Codex compaction boundary markers with no provider-visible content
- Retry the same strict compaction request when a custom model omits `submit_compaction`
- Add regression coverage for DUCX replay, macOS window policy, installation progress, and quota layout

<!-- codex-mixin:en:end -->

<!-- codex-mixin:zh-Hans:start -->
## v0.5.1

这是针对新版 DUCX 的紧急兼容修复。

- 改用显式 loopback endpoint 捕获 DUCX 原生认证 header，不再依赖新版 DUCX 已绕过的 HTTP proxy 环境变量
- 恢复百度 OneAPI 模型发现和推理请求
- 保留旧认证载体的 forward proxy 兼容能力

<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.5.1

這是針對新版 DUCX 的緊急相容性修正。

- 改用明確的 loopback endpoint 擷取 DUCX 原生驗證 header，不再依賴新版 DUCX 已略過的 HTTP proxy 環境變數
- 恢復百度 OneAPI 模型探索和推理請求
- 保留舊驗證載體的 forward proxy 相容能力

<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.5.1

This is an emergency compatibility fix for the latest DUCX release.

- Capture native DUCX authentication headers through an explicit loopback endpoint instead of HTTP proxy environment variables that the latest DUCX bypasses
- Restore Baidu OneAPI model discovery and inference requests
- Preserve forward proxy compatibility for legacy authentication carriers

<!-- codex-mixin:en:end -->

<!-- codex-mixin:zh-Hans:start -->
## v0.5.0

这是 Codex Mixin 的界面与使用体验大版本。macOS 控制中心完成 SwiftUI 重构，Linux、SSH 和远端开发机获得可用鼠标与键盘操作的全屏 TUI；两套界面现在共享同一套 Provider、模型、应用和系统管理能力。

### 全屏 TUI

- 无参数运行 `codex-mixin` 即进入主界面；首次启动自动安装到 `~/.local/bin/codex-mixin` 并打开 Setup
- 提供 Home、Setup、Providers、Models、Speed、Fusion、Apps、System 和 Logs 完整页面
- 支持新增、编辑、删除 Provider，配置 API Key、DUCX 扫码认证、辅助模型和百度数据上报
- 支持模型发现、多选、保存和测速，并展示 TTFT、吞吐、Token、额度及健康状态
- 可安装或恢复 Codex、Claude Code 和 DSH，管理 gateway、升级、修复及诊断日志
- 支持鼠标点击、键盘导航和全屏二级工作区；原有 subcommand、`--json` 与 `--no-tui` 继续用于脚本和 macOS App

### macOS 控制中心

- Provider 设置、模型测速、Fusion、安装进度、Codex 安装、About、用量和诊断界面迁移到 SwiftUI
- 重新设计 Provider 设置和模型选择体验，增加响应性能、平均 TTFT 与输出吞吐展示
- 使用 macOS 26 Liquid Glass，同时为 macOS 13.1–15 提供原生材质兼容界面
- 最低系统版本保持 macOS 13.1 Ventura，不要求升级到 macOS 26

### 稳定性

- 修复官方 WebSocket 与 CLI JSON 转发中的 timing 数据采集和展示
- 修复 TUI 操作结果丢失、长任务阻塞、Tab 导航、鼠标命中和 DUCX 二维码显示
- 改进 Provider throughput 聚合并排除异常 timing 样本

<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.5.0

這是 Codex Mixin 的介面與使用體驗大版本。macOS 控制中心完成 SwiftUI 重構，Linux、SSH 和遠端開發機獲得可用滑鼠與鍵盤操作的全螢幕 TUI；兩套介面現在共享同一套 Provider、模型、應用程式和系統管理能力。

### 全螢幕 TUI

- 不帶參數執行 `codex-mixin` 即進入主介面；首次啟動自動安裝到 `~/.local/bin/codex-mixin` 並開啟 Setup
- 提供 Home、Setup、Providers、Models、Speed、Fusion、Apps、System 和 Logs 完整頁面
- 支援新增、編輯、刪除 Provider，設定 API Key、DUCX 掃碼認證、輔助模型和百度資料上報
- 支援模型探索、多選、儲存和測速，並顯示 TTFT、吞吐、Token、額度及健康狀態
- 可安裝或還原 Codex、Claude Code 和 DSH，管理 gateway、升級、修復及診斷日誌
- 支援滑鼠點選、鍵盤導覽和全螢幕次級工作區；原有 subcommand、`--json` 與 `--no-tui` 繼續用於腳本和 macOS App

### macOS 控制中心

- Provider 設定、模型測速、Fusion、安裝進度、Codex 安裝、About、用量和診斷介面遷移到 SwiftUI
- 重新設計 Provider 設定和模型選擇體驗，增加回應效能、平均 TTFT 與輸出吞吐顯示
- 使用 macOS 26 Liquid Glass，同時為 macOS 13.1–15 提供原生材質相容介面
- 最低系統版本保持 macOS 13.1 Ventura，不要求升級到 macOS 26

### 穩定性

- 修正官方 WebSocket 與 CLI JSON 轉送中的 timing 資料收集和顯示
- 修正 TUI 操作結果遺失、長任務阻塞、Tab 導覽、滑鼠命中和 DUCX QR code 顯示
- 改善 Provider throughput 彙總並排除異常 timing 樣本

<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.5.0

This is a major interface and usability release. The macOS control center has moved to SwiftUI, while Linux, SSH, and remote development environments now have a full-screen TUI with mouse and keyboard control. Both interfaces manage the same providers, models, applications, and system services.

### Full-screen TUI

- Run `codex-mixin` without arguments to open the control center; the first launch installs it to `~/.local/bin/codex-mixin` and opens Setup
- Use complete Home, Setup, Providers, Models, Speed, Fusion, Apps, System, and Logs pages
- Add, edit, and remove providers, including API keys, DUCX QR authentication, auxiliary models, and Baidu reporting settings
- Discover, select, save, and benchmark models with TTFT, throughput, token, quota, and health data
- Install or restore Codex, Claude Code, and DSH, and manage the gateway, updates, repairs, and diagnostics
- Use mouse input, keyboard navigation, and full-screen secondary workspaces; existing subcommands, `--json`, and `--no-tui` remain available for automation and the macOS app

### macOS control center

- Move provider settings, model benchmarks, Fusion, installation progress, Codex installation, About, usage, and diagnostics to SwiftUI
- Redesign provider settings and model selection, including response performance, average TTFT, and output throughput
- Use Liquid Glass on macOS 26 with a native material interface on macOS 13.1–15
- Keep macOS 13.1 Ventura as the minimum supported version; macOS 26 is not required

### Reliability

- Fix timing collection and display for official WebSocket traffic and CLI JSON forwarding
- Fix lost TUI operation results, blocking tasks, Tab navigation, mouse hit testing, and DUCX QR rendering
- Improve provider throughput aggregation and reject anomalous timing samples

<!-- codex-mixin:en:end -->

<!-- codex-mixin:zh-Hans:start -->
## v0.4.4

本版本改进了 Provider、gateway 和 macOS 设置体验，并修复了百度代码使用上报。

### Provider 与 gateway

- 修复自定义 Provider 的 compact 输出，补齐百度 Responses compact 路径验证
- 明确 Provider capability probe 的触发和结果，模型刷新不再混入过期能力状态
- gateway 优雅停止超时后会结束残留 daemon，避免服务进程卡住

### macOS

- 修复模型选择器中的 token 使用量滚动位置
- 分离模型保存和 benchmark 操作，并在新 benchmark 开始前清理旧结果

### 百度上报可靠性

- 修复 transcript 在 `SessionStart` 时上传的问题；上游 session 尚未建立时不再触发 `Session meta not found`
- 只有 `upload/query` 成功后，才上报该 session 的代码生成、代码接受和 transcript
- 缺少成功 query 的历史 session 会记录明确的 skip 原因，不再发送无效的 code 或 transcript 请求
- Mixin 直接维护上报生命周期；DUCX 只用于获取短期 report client token
- 上报响应日志会脱敏签名文件 URL，避免短期授权参数写入本机日志

### 验证

- `cargo fmt --all -- --check`
- `cargo test --locked --all-targets`
- `cargo clippy --all-targets -- -D clippy::cognitive_complexity -D clippy::redundant_closure_for_method_calls -D clippy::redundant_pattern_matching -D warnings`

<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.4.4

本版本改善 Provider、gateway 和 macOS 設定體驗，並修正百度程式碼使用上報。

### Provider 與 gateway

- 修正自訂 Provider 的 compact 輸出，補齊百度 Responses compact 路徑驗證
- 明確 Provider capability probe 的觸發和結果，模型更新不再混入過期能力狀態
- gateway 優雅停止逾時後會結束殘留 daemon，避免服務程序卡住

### macOS

- 修正模型選擇器中的 token 使用量捲動位置
- 分離模型儲存和 benchmark 操作，並在新 benchmark 開始前清除舊結果

### 百度上報可靠性

- 修正 transcript 在 `SessionStart` 時上傳的問題；上游 session 尚未建立時不再觸發 `Session meta not found`
- 只有 `upload/query` 成功後，才上報該 session 的程式碼生成、程式碼接受和 transcript
- 缺少成功 query 的歷史 session 會記錄明確的 skip 原因，不再發送無效的 code 或 transcript 請求
- Mixin 直接維護上報生命週期；DUCX 只用於取得短期 report client token
- 上報回應日誌會遮蔽簽名檔案 URL，避免短期授權參數寫入本機日誌

### 驗證

- `cargo fmt --all -- --check`
- `cargo test --locked --all-targets`
- `cargo clippy --all-targets -- -D clippy::cognitive_complexity -D clippy::redundant_closure_for_method_calls -D clippy::redundant_pattern_matching -D warnings`

<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.4.4

This release improves provider, gateway, and macOS settings behavior, and fixes Baidu code-usage reporting.

### Provider and gateway

- Fix compact output for custom providers and validate the Baidu Responses compact path
- Make provider capability probes and their results explicit so model refresh does not mix in stale capability state
- Terminate a remaining daemon when graceful gateway shutdown times out

### macOS

- Fix the token-usage scroll position in the model selector
- Separate model saving from benchmark actions and clear old results before a new benchmark

### Baidu reporting reliability

- Stop uploading transcripts at `SessionStart`, which could send them before the upstream session exists and trigger `Session meta not found`
- Upload code generation, code acceptance, and transcripts only after `upload/query` succeeds for the session
- Log a clear skip reason for historical sessions without a successful query instead of sending invalid code or transcript requests
- Keep the reporting lifecycle in Mixin; use DUCX only to obtain a short-lived report client token
- Redact signed file URLs in report responses before writing them to local logs

### Validation

- `cargo fmt --all -- --check`
- `cargo test --locked --all-targets`
- `cargo clippy --all-targets -- -D clippy::cognitive_complexity -D clippy::redundant_closure_for_method_calls -D clippy::redundant_pattern_matching -D warnings`

<!-- codex-mixin:en:end -->

<!-- codex-mixin:zh-Hans:start -->
## v0.4.3

- 新增官方 Provider 默认展示，开箱即可选择
- 优化 Provider 探测逻辑，支持手动调整 Provider 顺序
- 支持 DSH 模型安装和思考强度选择
- 优化模型刷新、额度与用量状态显示
- 改进 Gateway 稳定性和 macOS App 更新流程

<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.4.3

- 新增官方 Provider 預設顯示，開箱即可選擇
- 優化 Provider 探測邏輯，支援手動調整 Provider 順序
- 支援 DSH 模型安裝和思考強度選擇
- 優化模型更新、額度與用量狀態顯示
- 改進 Gateway 穩定性和 macOS App 更新流程

<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.4.3

- Show official providers by default for quick setup
- Improve provider discovery and allow manual provider ordering
- Support DSH model installation and reasoning intensity selection
- Improve model refresh, quota, and usage status display
- Improve gateway stability and macOS app update flow

<!-- codex-mixin:en:end -->

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
