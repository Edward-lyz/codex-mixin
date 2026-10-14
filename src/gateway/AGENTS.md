# 单请求网关执行规范

继承上级规范。这里解析路由、构建 RequestPlan、执行普通请求并观察缓存，是 server/fusion 复用的执行入口。

- `router` 解析目标，`plan` 表达请求计划，`executor` 执行；请求输入和 provider 元数据显式传入，不读取上层状态。
- 不依赖入站服务器、CLI 或多模型编排层；Fusion 目标交回编排入口，普通执行器不递归执行面板。
- 上游发送复用 upstream，转换复用 protocol，媒体处理复用 images；不建立第二套 HTTP client/认证栈。
- 保留模型级 endpoint、reasoning、工具、输出上限和 cache usage 语义；只按明确兼容策略改写请求。
- SSE 增量处理，复用 bytes/client；缓存与会话观测容量有界，不能在热路径全量扫描状态文件。
- 验证官方/自定义路由、非法目标、取消、输出上限与缓存前缀，运行 gateway 相关测试、`cargo test --locked --test gateway_http` 和根级门禁。
