# Realtime 连接规范

继承上级规范。本目录与 `../realtime.rs` 处理 realtime/live/sideband 路由及双向 transport。

- `routing` 只解析目标与 URL/query 映射，`transport` 处理连接与转发；不将模型选择散入 frame 循环。
- 官方/自定义调用位置与 sideband call ID 按既有协议重写，保留未知但允许的 query，拒绝非法目标。
- 在连接建立前执行网关认证；上游凭据仅交给对应目标，不回传客户端。
- 限制 call body、frame 和等待时间，处理双向关闭/取消，避免孤立转发任务。
- 协议错误按连接阶段返回，建立失败不能伪装成成功升级。
- 验证 query 隔离、Location 重写、sideband、连接拒绝和断开，运行 `cargo test --locked --test query_isolation`、WS 相关测试与根级门禁。
