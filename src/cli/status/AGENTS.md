# 状态查询规范

继承上级规范。本目录与 `../status.rs` 提供配置、使用量、额度和 OpenCode Go 状态。

- 默认查询只读；不隐式安装、刷新主配置或修复客户端。
- 区分 disabled、unconfigured、unknown、unavailable 和实际零值；网络失败不能输出零额度或健康状态。
- 多来源状态保留来源、时间和错误，额度单位与统计时间窗沿用类型定义，不能混合展示。
- 缓存明确有效期；耗时与并发有界，避免 UI 高频刷新造成重复探测。
- CLI 输出与 GUI 所用 JSON 同源；敏感配置仅呈现脱敏摘要。
- 运行 quota、runtime、status 相关 Rust 测试，验证离线、过期缓存、禁用 provider 和查询零写入，再执行根级门禁。
