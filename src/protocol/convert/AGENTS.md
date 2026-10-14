# Responses 到 Anthropic 转换规范

继承上级规范。`request`/`content` 处理请求与内容，`thinking` 处理思考语义，`tool_map`/`tools` 处理工具映射。

- 保留消息次序、system 内容、tool call ID 与 tool result 配对；不能把工具输出转成普通用户文本。
- thinking、signature、reasoning effort 与模型能力按既有明确规则处理，不伪造签名。
- 图片和多模态内容保留协议类型与顺序；不在纯转换中下载资源或压缩图片。
- 工具名/schema 映射稳定且可逆，避免跨工具冲突；非法字段返回明确错误。
- 验证多轮工具、缺字段、空内容、thinking 与图片组合，运行 `cargo test --locked protocol::convert`，前缀变化补 E2E。
