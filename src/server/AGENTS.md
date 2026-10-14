# 入站 HTTP/WS Adapter 规范

继承上级规范。这里拥有路由、网关认证、入站解析、连接生命周期和响应封装；状态对象只组合能力。

- 普通请求交给 gateway，多模型请求交给 fusion；官方透传等既有路径使用 upstream，不能另写发送/认证栈。
- 在读取大 body 或升级 WS 前校验认证和限制；统一处理请求大小、解压后体积和帧/消息上限。
- handler 只编排输入/输出，不承载 provider 选择规则、目录生成或配置持久化。
- HTTP 错误使用 `GatewayError`；客户端响应不暴露内部路径、认证和完整错误链。
- SSE/WS 增量转发并尊重背压；取消、断连与 server shutdown 收敛上游连接和任务。
- 请求/连接状态不能跨用户、provider 或 session 复用；header/query 映射使用明确允许规则。
- 运行 server 单测、`cargo test --locked --test gateway_http`、`cargo test --locked --test websocket_proxy` 和根级门禁；协议/缓存变更补 E2E。
