<!-- codex-mixin:zh-Hans:start -->
## v0.3.4

- 新增完整的多 Provider 管理：可以同时配置多个供应商，每个 Provider 独立保存 API Key、模型发现缓存、启停状态和模型 allowlist；第三方模型使用稳定的 Provider 后缀并精确路由，Fusion、模型测速、额度查询和图片生成也都按 Provider 隔离。
- 新增 OpenCode Go 预设，内置首批可用模型并支持在线刷新；新增 Custom Provider 时只需填写站点名称、API 地址和 API Key，常见完整接口地址会自动拆分根地址、协议和路径，不再要求用户选择 `/v1/messages` 等内部接口类型。
- 旧版单 Provider `config.json` 会在首次修改时无损迁移并保留备份；恢复额度展示和旧配置工作流。运行配置不再受 `CODEX_GATEWAY_*` 环境变量覆盖，避免用户 shell 环境意外干扰 App。
- 新增 CLI `doctor` 和 App“自动检测”，检查配置格式与权限、Provider 可用性、模型发现、网关进程、Codex 集成和日志状态；Codex 安装还会通过 `codex doctor` 与 `codex debug models` 校验最终加载的 Provider 和模型。
- Custom Provider 会并发尝试 New API、Sub2API、OpenRouter 等常见只读额度接口；菜单使用用户自定义站点名称展示各 Provider 额度。修复多个额度条宽度不一致、菜单刷新竞态以及删除 Provider 后编号不连续的问题。
- Baidu OneAPI 的额度用户名改为必填，修复其模型与额度显示，并在模型选择列表新增倍率列。
- 修复供应商配置表单悬浮在其他 App 之上、切换窗口时遮挡浏览器、部分 macOS 版本文字被裁切，以及新增 Provider 后可能导致菜单栏 App 退出的问题。更新供应商后会明确提示重启 Codex App。
- Web Search 能力探测改为后台进行，不再阻塞首次安装；Linux x86_64 与 ARM64 发布包改为静态 musl 构建，减少目标机器对系统运行库的依赖。
- `gateway.log` 新增 App 操作、子进程、Catalog 刷新和 Codex 安装校验的逐步记录，包括操作 ID、耗时、退出码、Catalog 路径/模式/模型数量；日志统一脱敏 API Key、Token 和密码并固定为 `0600` 权限。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.4

- 新增完整的多 Provider 管理：可以同時設定多個供應商，每個 Provider 獨立儲存 API Key、模型探索快取、啟停狀態和模型 allowlist；第三方模型使用穩定的 Provider 後綴並精確路由，Fusion、模型測速、額度查詢和圖片生成也都按 Provider 隔離。
- 新增 OpenCode Go 預設，內建首批可用模型並支援線上更新；新增 Custom Provider 時只需填寫站點名稱、API 位址和 API Key，常見完整端點會自動拆分根位址、協定和路徑，不再要求使用者選擇 `/v1/messages` 等內部端點類型。
- 舊版單 Provider `config.json` 會在首次修改時無損遷移並保留備份；恢復額度顯示和舊設定流程。執行設定不再受 `CODEX_GATEWAY_*` 環境變數覆蓋，避免使用者 shell 環境意外干擾 App。
- 新增 CLI `doctor` 和 App「自動檢測」，檢查設定格式與權限、Provider 可用性、模型探索、Gateway 程序、Codex 整合和日誌狀態；Codex 安裝還會透過 `codex doctor` 與 `codex debug models` 驗證最終載入的 Provider 和模型。
- Custom Provider 會並行嘗試 New API、Sub2API、OpenRouter 等常見唯讀額度端點；選單使用使用者自訂站點名稱顯示各 Provider 額度。修復多個額度條寬度不一致、選單更新競態以及刪除 Provider 後編號不連續的問題。
- Baidu OneAPI 的額度使用者名稱改為必填，修復其模型與額度顯示，並在模型選擇清單新增倍率欄。
- 修復供應商設定表單懸浮在其他 App 之上、切換視窗時遮擋瀏覽器、部分 macOS 版本文字被裁切，以及新增 Provider 後可能導致選單列 App 結束的問題。更新供應商後會明確提示重新啟動 Codex App。
- Web Search 能力探索改為背景執行，不再阻塞首次安裝；Linux x86_64 與 ARM64 發布包改為靜態 musl 建置，減少目標機器對系統執行庫的依賴。
- `gateway.log` 新增 App 操作、子程序、Catalog 更新和 Codex 安裝驗證的逐步記錄，包括操作 ID、耗時、退出碼、Catalog 路徑/模式/模型數量；日誌統一遮蔽 API Key、Token 和密碼並固定為 `0600` 權限。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.4

- Added full multi-provider management. Multiple providers can now coexist with independent credentials, discovery caches, enablement, and model allowlists. Stable provider-suffixed model slugs are routed exactly, and Fusion, benchmarks, quota checks, and image generation are provider-aware.
- Added an OpenCode Go preset with a seeded model catalog and live refresh. A Custom Provider now requires only a site name, API address, and API key; common full endpoint URLs are split into the appropriate base URL, protocol, and path without asking users to choose internal endpoint types such as `/v1/messages`.
- Legacy single-provider `config.json` files are migrated without data loss and backed up on the first mutation. Quota visualization and legacy configuration workflows are restored. Runtime behavior no longer accepts `CODEX_GATEWAY_*` environment overrides, preventing shell state from unexpectedly changing the App.
- Added CLI `doctor` and the App's Automatic Diagnostics entry point for config format and permissions, provider reachability, model discovery, gateway runtime, Codex integration, and log health. Codex installation now also runs `codex doctor` and `codex debug models` to validate the effective provider and loaded model catalog.
- Custom Providers concurrently probe common read-only quota endpoints used by New API, Sub2API, OpenRouter, and similar services. Menus use each provider's custom display name. Fixed uneven quota track widths, menu refresh races, and generated provider IDs not being compacted after deletion.
- Made the quota username mandatory for Baidu OneAPI, fixed its model and quota presentation, and added a model-rate column to the model picker.
- Fixed provider forms floating above unrelated apps, blocking the browser after switching windows, clipped text on older macOS releases, and a menu-bar App exit that could occur after adding a Provider. Provider updates now explicitly remind users to restart Codex.
- Moved Web Search capability probing to the background so first installation is no longer blocked. Linux x86_64 and ARM64 release artifacts now use static musl builds to reduce target-system runtime dependencies.
- Expanded `gateway.log` with step-by-step App operation, child-process, catalog refresh, and Codex installation validation records, including operation IDs, durations, exit codes, and catalog path/mode/model counts. API keys, tokens, and passwords are redacted, and the log is forced to `0600` permissions.
<!-- codex-mixin:en:end -->
