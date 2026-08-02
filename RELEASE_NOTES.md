<!-- codex-mixin:zh-Hans:start -->
## v0.3.12

### 极致 prompt 前缀缓存

- 新增 provider 请求的前缀缓存契约：从真正发往上游的字节推导 system、工具、reasoning 配置与逐条消息的形状，跨轮比对后报告缓存在哪里丢失（`cold_start`、`append_only`、`tail_rewritten`、`system_changed`、`tools_changed`、`config_changed`、`turn_rewritten`、`history_truncated`）。
- 缓存丢失以 WARN 记录并附带 `reused_turns` 与 `reused_bytes`；每轮完整轨迹用 `RUST_LOG=codex_mixin=debug` 查看，网关日志级别现在可由 `RUST_LOG` 控制，默认仍为 info。
- 网关日志重定向到文件时不再写入 ANSI 颜色码，只有交互式终端才着色。
- 工具返回的图片只在模型尚未看过的那一轮内联（压缩到最长边 1568px），之后回放为稳定占位符。截图与视觉工具继续可用，而历史不再永久携带图片字节。
- 图片被省略时一定留下可见标记，模型不会把残留文本误认为完整的工具结果。
- 同一 session 下 fusion 各面板与并发子任务的不同 prompt 分别独立跟踪，不会互相误报缓存丢失。
- 新增端到端回归，在真实网关上逐字校验上游请求，锁定以上行为。

### 上游兼容性修复

- OpenAI Chat 兼容 provider 不再收到内嵌在 `tool` 消息里的图片；图片改为紧随该批工具结果之后的一条 user 消息，assistant `tool_calls` 与对应 `tool` 结果保持相邻。
- 修正 Baidu OneAPI 的 token 用量映射，`cache_read_input_tokens` 与 `cache_creation_input_tokens` 现在正确计入输入 token 与缓存命中数。
- Anthropic 思考内容跨轮保留（含 signature），thinking 模型的多轮对话不再丢失推理上下文。
- 修正 Anthropic web search 的查询提取，并跳过空的 web search 调用。

### macOS 菜单栏

- 网关启停合并进状态标题里的开关，菜单更紧凑。
- 打开菜单时刷新开关状态。
- 支持 Cmd+W 关闭窗口。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.12

### 極致 prompt 前綴快取

- 新增 provider 請求的前綴快取契約：從真正送往上游的位元組推導 system、工具、reasoning 設定與逐條訊息的形狀，跨輪比對後回報快取在哪裡失效（`cold_start`、`append_only`、`tail_rewritten`、`system_changed`、`tools_changed`、`config_changed`、`turn_rewritten`、`history_truncated`）。
- 快取失效以 WARN 記錄並附上 `reused_turns` 與 `reused_bytes`；每輪完整軌跡用 `RUST_LOG=codex_mixin=debug` 檢視，閘道日誌層級現在可由 `RUST_LOG` 控制，預設仍為 info。
- 閘道日誌重新導向到檔案時不再寫入 ANSI 顏色碼，只有互動式終端才著色。
- 工具回傳的圖片只在模型尚未看過的那一輪內嵌（壓縮到最長邊 1568px），之後回放為穩定佔位符。截圖與視覺工具仍然可用，而歷史不再永久攜帶圖片位元組。
- 圖片被省略時一定留下可見標記，模型不會把殘留文字誤認為完整的工具結果。
- 同一 session 下 fusion 各面板與並行子任務的不同 prompt 各自獨立追蹤，不會互相誤報快取失效。
- 新增端到端回歸測試，在真實閘道上逐字驗證上游請求，鎖定以上行為。

### 上游相容性修正

- OpenAI Chat 相容 provider 不再收到內嵌在 `tool` 訊息裡的圖片；圖片改為緊接該批工具結果之後的一條 user 訊息，assistant `tool_calls` 與對應 `tool` 結果保持相鄰。
- 修正 Baidu OneAPI 的 token 用量對應，`cache_read_input_tokens` 與 `cache_creation_input_tokens` 現在正確計入輸入 token 與快取命中數。
- Anthropic 思考內容跨輪保留（含 signature），thinking 模型的多輪對話不再遺失推理脈絡。
- 修正 Anthropic web search 的查詢擷取，並略過空的 web search 呼叫。

### macOS 選單列

- 閘道啟停合併進狀態標題裡的開關，選單更精簡。
- 開啟選單時刷新開關狀態。
- 支援 Cmd+W 關閉視窗。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.12

### Aggressive prompt prefix caching

- Added a prefix cache contract for provider requests. The cache-relevant shape of the system prompt, tool configuration, reasoning configuration and every message is derived from the bytes actually sent upstream, compared across turns, and reported when a session loses its prefix: `cold_start`, `append_only`, `tail_rewritten`, `system_changed`, `tools_changed`, `config_changed`, `turn_rewritten`, `history_truncated`.
- Cache loss is logged at WARN with `reused_turns` and `reused_bytes`. The full per-turn trail is available with `RUST_LOG=codex_mixin=debug`; the gateway log level now honours `RUST_LOG` and still defaults to info.
- A redirected gateway log no longer contains ANSI colour escapes; only an interactive terminal is coloured.
- A tool image is inlined only on the turn the model has not answered yet, compressed to a 1568 px longest side, and replayed as a stable marker afterwards. Screenshots and vision tools keep working while history stops carrying image bytes forever.
- An omitted image always leaves a visible marker, so the model cannot read the surrounding text as the complete tool result.
- Fusion panels and concurrent subagents that share one session id are tracked as separate lineages, so their interleaved prompts no longer look like cache loss.
- Added an end-to-end regression that checks the upstream request bytes through a real gateway to pin all of the above.

### Upstream compatibility fixes

- OpenAI Chat compatible providers no longer receive images inside `tool` messages. Images move into a user message placed right after the tool run, keeping assistant `tool_calls` adjacent to the `tool` results they pair with.
- Fixed Baidu OneAPI token usage mapping so `cache_read_input_tokens` and `cache_creation_input_tokens` are counted in the input tokens and cached token totals.
- Anthropic thinking content, including its signature, is preserved across turns, so multi-turn conversations with thinking models keep their reasoning context.
- Fixed Anthropic web search query extraction and skipped empty web search calls.

### macOS menu bar

- The gateway start and stop actions are merged into a switch in the status header, and the menu is more compact.
- The switch state refreshes when the menu opens.
- Cmd+W closes windows.
<!-- codex-mixin:en:end -->
