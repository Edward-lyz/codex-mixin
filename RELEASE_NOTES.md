<!-- codex-mixin:zh-Hans:start -->
## v0.2.25

- Fusion 新增 `show_intermediate_results` 开关；关闭时只显示 Final 回答，Panel 和 Judge 仍在后台参与生成。
- 开启中间结果时，Panel 输出按配置顺序合并为可折叠表格，并使用统一的 `Fusion · Panel Results`、`Fusion · Judge Result`、`Fusion · Final Answer` 标题。
- Baidu OneAPI 改用 `/openapi/v2/available_models` 作为权威模型源，修复 `auto` 模型未进入 Codex 模型列表的问题。
- 将过大的 Rust 与 macOS 源文件按职责拆分，并集中 provider suffix 与 Baidu/Fable 兼容判断，降低后续维护成本。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.2.25

- Fusion 新增 `show_intermediate_results` 開關；關閉時只顯示 Final 回答，Panel 與 Judge 仍會在背景參與生成。
- 開啟中間結果時，Panel 輸出會依設定順序合併成可摺疊表格，並使用統一的 `Fusion · Panel Results`、`Fusion · Judge Result`、`Fusion · Final Answer` 標題。
- Baidu OneAPI 改用 `/openapi/v2/available_models` 作為權威模型來源，修正 `auto` 模型未進入 Codex 模型清單的問題。
- 將過大的 Rust 與 macOS 原始檔依職責拆分，並集中 provider suffix 與 Baidu/Fable 相容判斷，降低後續維護成本。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.2.25

- Added `show_intermediate_results` for Fusion profiles. Disabling it keeps only the Final answer visible while Panel and Judge still participate.
- When intermediate results are visible, Panel output is grouped into one collapsible table followed by consistently titled Judge and Final sections.
- Made `/openapi/v2/available_models` authoritative for Baidu OneAPI, fixing the missing `auto` model in the Codex model picker.
- Split oversized Rust and macOS sources by responsibility and centralized provider-suffix and Baidu/Fable compatibility checks.
<!-- codex-mixin:en:end -->
