# Prompt Cache 前缀规范

继承上级规范。本目录与 `../cache_shape.rs` 从 provider 实际可见请求推导前缀形状并记录变化原因。

- `shape` 分析准确序列化字节，`tracking` 管理有界会话观测；不能用语义相同替代字节相同。
- system、tools、历史消息次序与 append-only 契约保持明确；模型/provider/session 隔离，不能跨会话比较。
- 观测不能改写请求以“提高命中”，不能根据推测填充上游 cache usage。
- 会话、shape 数量和最小判定门槛使用已有常量；不无界保存完整 prompt 或泄露内容。
- 缓存失效报告具体区域/原因，unknown 与实际 miss 区分。
- 修改后运行 cache_shape 相关测试和 `./scripts/e2e_prompt_cache.sh`，核对实际 upstream payload 与多轮前缀行为，再执行根级门禁。
