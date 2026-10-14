# 上游流到 Responses 事件规范

继承上级规范。这里把 Anthropic/Chat 流映射成 Responses SSE，`state.rs` 持有显式流状态。

- 每个响应/item/content block 维护稳定 ID/index；开始、增量、结束和完成事件顺序由状态转换约束。
- tool arguments 和 reasoning 增量可被任意 chunk 拆开，按工具索引正确归并；不把部分 JSON 提前当成完整参数。
- 文本、thinking/signature、web search 和工具块保持各自语义，usage 来自真实上游。
- EOF、取消、上游错误与正常完成分开；不重复完成，不用空成功掩盖截断。
- 保持状态与缓冲有界，不持有 provider/服务对象，不在 mapper 发网络请求。
- 验证乱序/缺失字段、交错工具、空 delta、usage、错误和多 chunk，运行 `cargo test --locked protocol::openai_events` 及根级门禁。
