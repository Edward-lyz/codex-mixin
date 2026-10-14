# Responses 到 Chat Completions 转换规范

继承上级规范。这里只生成 Chat Completions payload，不执行请求。

- `request` 管理顶层字段，`content` 管理消息/多模态，`tools` 管理函数工具；保持角色、顺序、工具 ID 与结果关系。
- reasoning、token limit、stream options 等模型差异按已验证能力处理，不按名称猜测支持字段。
- 保留 system/tools 前缀和用户历史，避免每轮重排或重复追加内容。
- 不支持的协议数据按明确规则报错或转换；不无声丢弃工具、图片与思考内容。
- 验证多轮、空内容、工具并行、图片和模型字段差异，运行 `cargo test --locked protocol::openai_chat`；前缀改动补 E2E。
