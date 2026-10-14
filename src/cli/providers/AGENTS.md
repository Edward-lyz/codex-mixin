# Provider CLI 规范

继承上级规范。这里收集参数、调用 provider 用例并呈现结果，不拥有第二套配置规则。

- `management` 处理管理命令，`discovery` 处理输入与发现呈现，`models` 处理选择/探测入口；使用 application 的输入类型。
- provider 身份、端点、认证和模型合法性复用下层校验，不在 TUI/GUI 复制。
- JSON 输出保留模型来源、能力状态与失败阶段；新发现不等于已启用或已选择。
- 配置已保存后发现/探测失败，明确报告已提交；不补做未定义的隐式回滚。
- 密钥读取遵守 CLI 输入机制，错误及测试输出脱敏；刷新不得丢失人工模型设定。
- 验证参数边界、选择保留、提交后失败和 JSON 输出，运行 provider 相关测试及 `cargo test --locked --test provider_commit_boundaries`。
