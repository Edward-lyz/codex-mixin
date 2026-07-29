<!-- codex-mixin:zh-Hans:start -->
## v0.3.7

- 修复连续使用 Browser 或 Computer Use 截图时，请求历史中的 base64 图片不断累积并触发网关或上游 Payload 限制的问题。网关现在允许最多 16 MiB 的 `/v1/responses` 请求，并在转发到自定义 Provider 前丢弃旧工具截图，只保留最新工具图片、所有文字、用户提供的图片和远程 URL。
- Provider 降级提示现在会明确列出具体 Provider 和原因，包括不可达模型、模型列表刷新失败、未配置 API Key，以及没有已启用的可用模型；CLI `status` 与 macOS 菜单栏均可直接查看。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.7

- 修正連續使用 Browser 或 Computer Use 截圖時，請求歷史中的 base64 圖片持續累積並觸發 Gateway 或上游 Payload 限制的問題。Gateway 現在允許最大 16 MiB 的 `/v1/responses` 請求，並在轉送至自訂 Provider 前丟棄舊工具截圖，只保留最新工具圖片、所有文字、使用者提供的圖片和遠端 URL。
- Provider 降級提示現在會明確列出具體 Provider 和原因，包括無法連線的模型、模型清單更新失敗、未設定 API Key，以及沒有已啟用的可用模型；CLI `status` 與 macOS 選單列皆可直接查看。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.7

- Fixed repeated Browser or Computer Use screenshots accumulating base64 image history until the gateway or upstream payload limit was exceeded. The gateway now accepts `/v1/responses` requests up to 16 MiB and discards older tool screenshots before forwarding to a custom Provider, while preserving the newest tool image, all text, user-provided images, and remote URLs.
- Provider degradation messages now identify the affected Provider and the exact reason, including unreachable models, model-list refresh failures, missing API keys, and no enabled usable models. The details are visible in both CLI `status` and the macOS menu bar.
<!-- codex-mixin:en:end -->
