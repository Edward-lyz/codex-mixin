<!-- codex-mixin:zh-Hans:start -->
## v0.3.0

- Fusion 中间结果升级为 Codex 原生交互式 `Fusion · Review`：Panel 报告以并列小卡片展示，可点击展开完整内容。
- Judge 输出重构为三个可点选的编号要点，分别覆盖共识与证据、分歧与缺口、建议的具体做法；标题和正文跟随当前用户请求的语言。
- Final 回答恢复为普通 Codex 流式消息，不再插入额外标题或模型说明；关闭 `show_intermediate_results` 时仍只显示 Final。
- visualization 文件限定在当前 Codex task 目录、拒绝路径逃逸并使用私有文件权限；目录不可用时自动回退到 Markdown 展示。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.0

- Fusion 中間結果升級為 Codex 原生互動式 `Fusion · Review`：Panel 報告以並列小卡片呈現，可點擊展開完整內容。
- Judge 輸出重構為三個可點選的編號要點，分別涵蓋共識與證據、分歧與缺口、建議的具體做法；標題與正文會跟隨目前使用者請求的語言。
- Final 回答恢復為一般 Codex 串流訊息，不再插入額外標題或模型說明；關閉 `show_intermediate_results` 時仍只顯示 Final。
- visualization 檔案限定在目前 Codex task 目錄、拒絕路徑逃逸並使用私有檔案權限；目錄不可用時自動回退到 Markdown 顯示。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.0

- Upgraded visible Fusion intermediates to a native interactive `Fusion · Review` in Codex, with compact side-by-side Panel cards that expand on click.
- Restructured Judge output into three selectable numbered points for consensus and evidence, tensions and gaps, and a concrete recommendation; titles and bodies follow the current user's language.
- Restored the Final answer to a normal streamed Codex message without an extra heading or model label. Disabling `show_intermediate_results` still keeps only the Final visible.
- Confined visualization files to the current Codex task directory, rejected path escapes, applied private file permissions, and retained a Markdown fallback when visualization is unavailable.
<!-- codex-mixin:en:end -->
