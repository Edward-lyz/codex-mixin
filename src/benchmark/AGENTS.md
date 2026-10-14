# 测速与成本规范

继承上级规范。这里拥有测速 run、目标、真实流式测量、取消与结果保存；GUI 只展示结果。

- `manager` 管理运行状态，`runner` 执行测量，`types` 定义状态与单位；不引用上层控制面。
- 首 token、吞吐、耗时、usage 与成本来自真实执行，使用单调时钟测持续时间；不能用估计值冒充测量值。
- 区分未运行、运行中、成功、失败、取消；部分失败保留每个模型的具体状态。
- 目标只包含当前允许的模型/provider，禁用 provider 不继续产生请求或费用。
- 并发、输出 token、超时及保存容量有界，取消及时停止任务；网络等待不持有状态锁。
- 验证状态转换、计量单位、禁用目标、失败与取消，运行 `cargo test --locked benchmark`、`cargo test --locked --test disabled_provider_costs` 和根级门禁。
