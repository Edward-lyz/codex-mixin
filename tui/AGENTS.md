# Terminal UI 壳规范

继承根规范。这里是 ratatui/crossterm 交互壳，由 main 组合，不成为 Rust 库或 CLI 私有模块的依赖。

- `view` 渲染，`events` 解释输入，`forms` 管理表单，`actions` 调用稳定 CLI；页面状态不承担 provider/网关业务规则。
- 调用 CLI 使用 `--no-tui`，避免递归进入 TUI；消费 JSON 与错误 contract，不访问 CLI 私有实现或直写配置。
- 终端必须可用才进入交互模式；退出/错误恢复 raw mode、鼠标、光标与 alternate screen。
- 渲染/事件循环不执行长时间阻塞操作；刷新与任务有界，保留取消和明确 busy 状态。
- 密码输入不回显、不写日志；显示按终端宽度和 Unicode 宽度裁剪，键盘操作保持一致。
- 运行 `cargo test --locked --bin codex-mixin`、根级架构/Rust 门禁；终端恢复与键盘操作在实际终端验证，不能只靠截图。
