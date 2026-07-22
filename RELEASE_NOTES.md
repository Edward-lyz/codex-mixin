<!-- codex-mixin:zh-Hans:start -->
## v0.3.2

- “安装到 Codex”面板新增账号模式选择：有 OpenAI / ChatGPT 账号时继续合并官方 GPT 与自定义模型；没有账号时可只安装自定义模型。
- 新增 `install-codex --custom-only`。该模式不依赖 `~/.codex/models_cache.json`，不会启用 OpenAI OAuth，并会自动选择一个可用的自定义默认模型。
- 未检测到官方模型缓存时，macOS 安装面板会默认选择“仅使用自定义模型”，避免首次安装被缺失缓存阻断。
- 自定义模式与 OAuth 模式现在是明确互斥的 CLI 选项；README 补充两种模式的配置形态和使用说明。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.2

- 「安裝到 Codex」面板新增帳號模式選擇：有 OpenAI / ChatGPT 帳號時繼續合併官方 GPT 與自訂模型；沒有帳號時可只安裝自訂模型。
- 新增 `install-codex --custom-only`。此模式不依賴 `~/.codex/models_cache.json`，不會啟用 OpenAI OAuth，並會自動選擇一個可用的自訂預設模型。
- 未偵測到官方模型快取時，macOS 安裝面板會預設選擇「僅使用自訂模型」，避免首次安裝因缺少快取而中斷。
- 自訂模式與 OAuth 模式現在是明確互斥的 CLI 選項；README 補充兩種模式的設定形式與使用說明。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.2

- Added an account-mode choice to the Install to Codex panel: users with an OpenAI / ChatGPT account can keep official GPT models alongside custom models, while users without an account can install custom models only.
- Added `install-codex --custom-only`. This mode does not depend on `~/.codex/models_cache.json`, does not enable OpenAI OAuth, and automatically selects an available custom default model.
- The macOS install panel now defaults to custom-only mode when no official model cache is detected, preventing first-time installation from being blocked by a missing cache.
- Custom-only and OAuth modes are now explicit, mutually exclusive CLI options, with both managed configuration shapes documented in the README.
<!-- codex-mixin:en:end -->
