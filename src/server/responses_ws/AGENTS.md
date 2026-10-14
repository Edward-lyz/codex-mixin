# Responses WebSocket 状态规范

继承上级规范。本目录与 `../responses_ws.rs` 分别处理官方透传与自定义模型 WS 请求。

- official/custom 的连接状态保持独立，模型/provider 切换必须按既有规则重置，不能重放别的会话历史。
- previous response、history、cache body 和事件序号具有明确生命周期；使用状态结构表达，不堆叠隐式布尔组合。
- 自定义请求复用普通网关执行能力，官方请求复用 upstream；不在 WS 实现复制 HTTP 的路由/认证。
- body、frame、等待任务有界；取消与断开清理 pending request 和上游连接。
- 错误/完成事件只发送一次，保留 tool ID 与流结束语义；不吞异常后返回正常完成。
- 验证多轮、切换、取消、空输出、工具增量与历史隔离，运行 server WS 测试、`cargo test --locked --test websocket_proxy` 和根级门禁。
