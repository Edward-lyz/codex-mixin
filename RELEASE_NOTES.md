<!-- codex-mixin:zh-Hans:start -->
## v0.3.6

- 强化 Fusion 规划与网关路由：仅在 Plan 模式的新用户轮次运行 Panel 与 Judge，Default/编码阶段及工具续跑直接交给 Final；HTTP、WebSocket 与 Fusion 统一使用同一套模型解析和上游执行路径，并补强 Panel 工具超时、进程树清理与事件一致性。
- 新增按模型声明的 reasoning 能力和完整 Ultra 支持。自定义模型可在 Codex 中显示 `Off / low / medium / high / xhigh / max / ultra`，Ultra 启用 multi-agent v2，并在远端安全映射为最高合法档位；明确不支持思考的模型不会收到 `reasoning`。
- 新发现的上游模型现在会自动加入 Codex allowlist；模型刷新入口移至“模型选择与测速”窗口，刷新、选择和测速集中在同一工作流。
- Provider 设置新增唯一的辅助模型上游，可接管 `codex-auto-review`、Realtime 与 Live 语音模型。OAuth 安装会在 Provider 缺少能力时回落官方路由；Custom-only 安装会根据实际模型能力置灰不可用选项，并提示仅支持自动审查或语音的情况。
- 新增 Realtime WebSocket、Live、WebRTC call 与 sideband 的完整透传和自定义 Provider 路由；Custom-only 缺少辅助模型时返回明确的 400，不再误打官方接口或转成 500。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.6

- 強化 Fusion 規劃與 Gateway 路由：僅在 Plan 模式的新使用者輪次執行 Panel 與 Judge，Default／編碼階段及工具續跑直接交給 Final；HTTP、WebSocket 與 Fusion 統一使用同一套模型解析和上游執行路徑，並補強 Panel 工具逾時、程序樹清理與事件一致性。
- 新增依模型宣告的 reasoning 能力和完整 Ultra 支援。自訂模型可在 Codex 中顯示 `Off / low / medium / high / xhigh / max / ultra`，Ultra 啟用 multi-agent v2，並在遠端安全映射為最高合法檔位；明確不支援思考的模型不會收到 `reasoning`。
- 新探索到的上游模型現在會自動加入 Codex allowlist；模型更新入口移至「模型選擇與測速」視窗，更新、選擇和測速集中在同一工作流程。
- Provider 設定新增唯一的輔助模型上游，可接管 `codex-auto-review`、Realtime 與 Live 語音模型。OAuth 安裝會在 Provider 缺少能力時回退官方路由；Custom-only 安裝會依實際模型能力停用不可用選項，並提示僅支援自動審查或語音的情況。
- 新增 Realtime WebSocket、Live、WebRTC call 與 sideband 的完整透傳和自訂 Provider 路由；Custom-only 缺少輔助模型時回傳明確的 400，不再誤打官方端點或轉成 500。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.6

- Hardened Fusion planning and gateway routing. Panel and Judge now run only for new user turns in Plan mode, while Default/coding turns and tool continuations go directly to Final. HTTP, WebSocket, and Fusion share model resolution and upstream execution, with stronger panel-tool timeouts, process-tree cleanup, and event consistency.
- Added model-specific reasoning capabilities and complete Ultra support. Custom models can expose `Off / low / medium / high / xhigh / max / ultra` in Codex; Ultra enables multi-agent v2 and is safely mapped to the highest legal upstream effort, while models that explicitly lack thinking never receive `reasoning`.
- Newly discovered upstream models now join the Codex allowlist automatically. Model refresh moved into the Model Selection & Benchmark window so refresh, selection, and benchmarking share one workflow.
- Added one configurable auxiliary-model Provider for `codex-auto-review`, Realtime, and Live voice models. OAuth installations fall back to official routing for missing capabilities; Custom-only installations disable unsupported choices and identify providers that support only auto review or voice.
- Added complete proxying and custom-Provider routing for Realtime WebSocket, Live, WebRTC calls, and sideband connections. Custom-only installations now return a clear 400 when an auxiliary model is unavailable instead of contacting official endpoints or surfacing a 500.
<!-- codex-mixin:en:end -->
