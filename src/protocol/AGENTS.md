# 协议转换规范

继承上级规范。这里负责 Responses、Chat Completions、Anthropic Messages 的数据转换与 SSE 状态，不发送网络请求、不读取配置或运行进程。

- 输入包含明确协议与模型语义；转换保留工具 ID、工具输出、thinking/reasoning、图片、usage 和结束原因。
- 不认识的格式在边界明确处理，不用空回复或正常结束掩盖解析失败；兼容规则需具体来源与 E2E。
- SSE 按任意 byte chunk 增量解码，覆盖 UTF-8 拆分、CRLF、多个事件、空 delta 与 EOF。
- 事件映射使用状态结构和 enum，维护稳定 item ID/index 及 created/delta/done/completed 次序。
- prompt 前缀不随轮次随意重排；协议转换修改同时检查工具 schema、历史和 cache shape。
- 请求转换、事件映射与 SSE 各自保持归属，不引入 provider runtime 或网关执行依赖。
- 运行 `cargo test --locked --test gateway_http` 与根级门禁；流式或前缀改动运行 `./scripts/e2e_prompt_cache.sh`。
