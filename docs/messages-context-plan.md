# 原生 Messages 上下文引擎接入计划

状态：2026-10-27，用户已批准计划内原生协议适配函数、流状态方法和测试函数；正在实施。用户随后授权由主代理验收后创建 PR。

## 当前行为

- `src/server/messages_http.rs` 对 AnthropicMessages provider 直接调用 upstream，跳过 GatewayExecutor / ContextRun。
- 其他 Messages 路线复用 Responses 转换；该转换主动丢弃历史 thinking，不能直接用于原生协议往返。
- `src/gateway/provider.rs` 的 Responses 出站路径未传入客户端 anthropic-beta；直接切换会改变已有行为。

## 实施范围

1. `src/server/messages_http.rs`：启用引擎且具备稳定会话身份时接入共享执行路径；关闭或无身份时保留原直通。
   除 `custom` / `function` 外的带类型原生工具以及历史中的未知原生内容块不能经共享 Responses 流无损表达，明确保留原始 JSON 直通；包括 hosted web search/fetch 和 code execution。Kiro 适配器不能透传这类原始块，明确返回错误；若已进入引擎的请求意外返回服务端工具块，流返回错误，不静默丢弃。
2. `src/gateway/plan.rs`、`src/gateway/provider.rs`、必要时 `src/gateway/executor.rs`：传递原生请求选项，复用已有发送、重试及上下文回路，保留 thinking、output_config、工具定义、metadata 和 beta；内部状态不得作为上游未知字段泄漏。
3. `src/protocol/anthropic_compat.rs`：补充原生往返适配，复用既有 thinking 编码；保留工具 ID、签名、停止原因与 usage，覆盖 SSE 和非流式返回。可能需要新增局部适配函数和流状态方法，不新建发送栈或通用框架。
4. `tests/gateway_http.rs` 与协议就近测试：先增加原生 Messages 引擎接入失败用例，再验证内部压缩回路、工具隐藏、会话隔离、签名往返、beta、关闭/无身份、错误及 413 重试。
5. 更新现有 README、上下文引擎文档和待完成的 SVG/Wiki，删除过时路线排除描述，完成此前文档交付。

## 风险与验证

- 必须保持原生 thinking 签名和工具配对；不能仅改路由条件。
- 保持增量流、取消和内部轮次上限；明确会话身份缺失行为，不能跨会话合并历史。
- 验证 fmt、严格 Clippy、架构正负例、全目标测试、client-auth 检查、缓存 E2E，以及 HTTP/WS 回归；长输出存临时日志，只报告有效结果。
- 不修改运行配置，不重启用户网关；验证后由主代理按用户最新授权整理提交并创建 PR，清理本任务临时产物。

## 规则确认

用户提供的 `/Users/liyanzhen/baidu/AGENTS.md` 要求新增自定义函数前取得同意。申请范围仅上述原生协议适配函数、必要流状态方法和回归测试函数；其余复用现有 ContextRun、协议 DTO、编码器和 upstream。
