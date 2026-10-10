# Chat Completions 到 Responses 的流式映射

Mixin 将兼容 Chat Completions 上游转换为 Responses 时，保留以下行为。
原生 Responses 上游继续透传，不使用这条转换路径。

## 工具调用

函数调用在收到完整的调用 ID 和名称后发送 `response.output_item.added`，
状态为 `in_progress`。随后每个参数片段产生
`response.function_call_arguments.delta`。item ID、call ID 和 output index
在整个调用中保持一致；并行调用以及上游复用 stream index 的不同调用分别保存。

调用完成前校验整个工具批次。函数参数必须为 JSON object，只有通过校验后
才发送 `response.function_call_arguments.done` 和 completed 的
`response.output_item.done`。delta 拼接、arguments.done 和最终 arguments
保留相同的空白和 UTF-8 文本。缺少 ID／名称、畸形参数或流中断会明确失败；
`length`／`content_filter` 返回 `response.incomplete`，不发送可执行的工具 done。

自定义工具的参数是包裹 input 的 JSON，图像工具的参数还需要插入路由信息。
这两类先发送 in_progress 的 added，完成校验后才发送解包或改写后的 payload
delta／done，避免把原始 JSON 当成自定义工具输入，或让最终参数与已发送的
delta 不一致。tool_search 保留其独立的 item 类型。

## 推理文本

`delta.reasoning_content` 和兼容的字符串 `delta.reasoning` 是上游已经返回的
原始推理文本。Mixin 将它们映射为 reasoning item 的
`content[].reasoning_text`，发送 `response.reasoning_text.delta`／`.done`，
保留空的 summary，不生成摘要，不改标为 commentary，也不从 token 数反推文本。
当两个字段同时存在时优先采用非空的 reasoning_content，避免重复输出。
`completion_tokens_details.reasoning_tokens` 作为 token 计数独立保留。

这是协议语义保真的 raw 映射，不保证所有客户端默认显示原始推理。
Codex TUI 的显示受 `show_raw_agent_reasoning` 控制；本次未验证 Desktop 的
raw 通道展示。原生上游的 reasoning summary 或加密推理状态不会被这条路径改写。

字段依据：[OpenAI Responses streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events)。
