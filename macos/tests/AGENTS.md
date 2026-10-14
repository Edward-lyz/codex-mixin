# Swift 壳测试规范

继承上级规范。这里是由 `scripts/test_*.sh` 编译/运行的 Swift 测试，不假设存在 Swift Package 或 XCTest target。

- 沿用 `@main` 和现有断言入口，测试真实 Support/Actions 行为；不要只复制生产公式。
- 子进程测试同时覆盖 stdout/stderr、大输出、非零退出及取消；JSON 解析不能混入进度日志。
- UI 相关验证焦点、状态更新、布局和窗口生命周期；静态字符串检查只能证明结构约束。
- 测试用临时目录、fixture 与受控进程，不触碰真实凭据/CLI 状态或改变用户登录启动。
- 新测试提供相应运行脚本，遵守既有 localization 准备过程；明确需要的 SDK 与最低系统版本。
- 运行对应脚本并按退出码报告结果；构建关联变更再执行 `./macos/build_app.sh`，未运行的交互检查明确列出。
