# 业务用例规范

继承根与 `src/AGENTS.md`。这里拥有完整操作的顺序与提交语义，是 CLI 和测试复用业务的入口；不负责参数解析、终端渲染或 GUI 状态。

- 用例接收具体依赖和已校验输入，返回领域结果；进度通过明确 callback/结果表达，不引入 UI crate 或终端打印。
- 组合 provider、config、catalog、clients、platform 等能力。下层不能为了调用用例而反向依赖这里；既有共享 DTO 的越界在相关修改中收敛到语义拥有方，不增加类似依赖。
- 主配置提交前的失败与提交后的失败分别使用 `OperationError` 的对应阶段；不得把部分成功包装成全部失败或全部成功。
- 客户端安装失败只撤销本次创建的凭据，不撤销原有凭据；后续同步只处理托管客户端。
- 生命周期决策在此表达，OS 执行交给 platform；新增业务先由 CLI 暴露，GUI 不另写一份。
- 验证提交前后错误、重复执行、客户端同步与生命周期边界；提交语义修改运行 `cargo test --locked --test provider_commit_boundaries`，并执行根级 Rust 门禁。
