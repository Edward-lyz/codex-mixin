# Windows 原生 GUI 壳规范

继承根规范。这里使用 C# / XAML / WPF（.NET 10 Fluent 主题），只消费真实 Rust CLI contract；业务策略由 CLI/core 负责。

- `CodexMixin/` 负责 WPF 页面与 Windows 窗口、托盘、登录启动；`Core/` 负责 CLI 进程与 JSON 适配。不得复制供应商校验、接入、路由或服务生命周期策略。
- 所有命令使用参数数组、`--no-tui` 与 `--json-errors`；查询使用 JSON，严格检查版本、字段及类型。未知状态不得展示为关闭、零用量或最新版本。
- 密钥只通过子进程环境中的 `@env:NAME` 传递，不写 argv、动作日志或截图。进度和失败输出必须脱敏。
- 窗口关闭收起到原生托盘；退出释放托盘、窗口回调和进程资源。GUI 登录启动与 CLI 网关自启分别管理，不覆盖 Windows 禁用的启动项。
- 安装与卸载保留用户配置；PATH 只修改本项目项。自包含分发随包携带 .NET runtime，不依赖用户预装 .NET。
- 自有类型和成员默认 internal/private；XAML 根类型、框架 override 和接口实现仅保留框架要求的 public 可见性，不形成业务公共 API。
- 图标从 macOS 唯一来源生成。构建先执行 `windows/scripts/generate-windows-assets.ps1`，不得提交生成资源、bin、obj 或 build。
- 验证由 `.github/workflows/windows.yml` 在 GitHub Windows runner 执行：`Tests` 用真实 Rust CLI 检查 `Core/` 的契约解析；`scripts/test-ui-cli.ps1` 用固定版本、校验哈希的微软 winapp 操作同次构建的真实 WPF 包，并用 CLI 核对结果。无可用桌面或操作失败不得跳过；失败上传脱敏窗口树、截图和日志。

在 Windows 仓库根运行：

```powershell
dotnet run --project windows/Tests/Tests.csproj -c Release -- target/release/codex-mixin.exe
dotnet publish windows/CodexMixin/CodexMixin.csproj -c Release -p:Platform=x64 -o windows/build/publish
Copy-Item target/release/codex-mixin.exe windows/build/publish/
powershell -ExecutionPolicy Bypass -File windows/scripts/package-windows.ps1
```
