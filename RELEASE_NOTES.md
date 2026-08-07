<!-- codex-mixin:zh-Hans:start -->
## v0.3.15

### 功能

- 为辅助模型上游提供托管 `imagegen` Skill，自定义生图无需 OpenAI Python SDK
- 勾选辅助模型上游时安装托管 Skill，取消勾选或卸载时恢复 Codex 官方 Skill
- 增加逐模型能力探测，并完善辅助模型 `auto` 别名路由

### 修复

- 修复 Anthropic 协议 Provider 因 Codex 自动注入 cached web search 而拒绝普通对话
- 修复 Codex 运行期间切换生图上游后 Skill 缓存未刷新的提示缺失
- 仅在 DUCC 认证模式实际变化时执行安装和登录，普通 Provider 保存不再重复登录
- 修复自定义 Provider 协议探测，并固定 OpenCode Go 使用 Responses 协议
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.15

### 功能

- 為輔助模型上游提供受管 `imagegen` Skill，自訂生圖不需要 OpenAI Python SDK
- 勾選輔助模型上游時安裝受管 Skill，取消勾選或解除安裝時還原 Codex 官方 Skill
- 增加逐模型能力探測，並完善輔助模型 `auto` 別名路由

### 修正

- 修正 Anthropic 協議 Provider 因 Codex 自動注入 cached web search 而拒絕一般對話
- 修正 Codex 執行期間切換生圖上游後缺少 Skill 快取重新載入提示
- 僅在 DUCC 認證模式實際變更時執行安裝和登入，一般 Provider 儲存不再重複登入
- 修正自訂 Provider 協議探測，並固定 OpenCode Go 使用 Responses 協議
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.15

### Features

- Add a managed `imagegen` skill for the auxiliary model provider without requiring the OpenAI Python SDK
- Install the managed skill when an auxiliary provider is selected, and restore the official Codex skill when it is cleared or uninstalled
- Add per-model capability probing and complete auxiliary `auto` alias routing

### Fixes

- Fix normal chats failing on Anthropic providers when Codex injects cached web search
- Explain that Codex must restart to reload its skill cache after the image provider selection changes
- Run DUCC installation and login only when the authentication mode changes
- Fix custom provider protocol detection and pin OpenCode Go to the Responses protocol
<!-- codex-mixin:en:end -->
