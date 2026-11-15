# Codex Mixin desktop

Windows、macOS、Linux 共用 `CodexMixin/` 中的 Avalonia 12 + TKCode.Arc 控制台。供应商、Agent、模型路由、上下文、用量和设置页面调用现有 Rust CLI；不建立第二份业务状态。

Arc 提供侧边栏、表单、弹簧展开、主题切换、路由画布和用量图表。标题栏可切换明暗主题与减少动态效果；Cmd/Ctrl+R 刷新，Cmd/Ctrl+B 收起侧边栏，Cmd/Ctrl+, 打开设置。未保存编辑在切页、刷新和退出前确认。

macOS 保留 AppKit 菜单栏、登录启动、轻量模式与 Sparkle 整包更新。菜单栏启动共享控制台的 `--hosted` 模式，关闭控制台释放界面进程；再次打开通过当前用户专用管道定位已有窗口。更新和退出菜单栏应用前要求关闭控制台，以保护草稿。

Windows 使用 Avalonia 原生托盘菜单，保留 WinSparkle 签名验证、整包更新、空闲安装和登录启动。Linux 提供独立桌面包与 `.desktop` 入口；原 CLI 静态包和浏览器控制面板继续用于无桌面环境。

## 构建

需要 Rust stable 和 .NET 10 SDK。Arc 包固定为 1.1.0，Avalonia 固定为 12.1.5；还原锁按目标平台保存。发布包自带 .NET 运行时。

macOS：

```sh
./desktop/build_app.sh
# 输出 dist/Codex Mixin.app
```

Windows（PowerShell，仓库根目录）：

```powershell
./windows/scripts/fetch-winsparkle.ps1
dotnet publish desktop/CodexMixin/CodexMixin.csproj -c Release -r win-x64 --self-contained true -o windows/build/publish
Copy-Item target/release/codex-mixin.exe windows/build/publish/
# 安装包仍使用 windows/scripts/package-windows.ps1
```

Linux：先构建对应 Rust CLI，再执行：

```sh
./desktop/package-linux.sh x86_64-unknown-linux-musl 0.8.4 dist/release
./scripts/package-linux-deb.sh x86_64-unknown-linux-musl amd64 0.8.4 dist/release
```

`--background` 从登录启动隐藏窗口；`--page=providers|agents|routing|context|usage|settings` 打开指定页面。可执行文件和 Rust CLI 必须放在同一目录。

## 验证和许可

本次迁移按用户要求仅在本地编译，不运行本地测试。现有 Rust、CLI contract 与 Windows 真机 UI Automation 由 CI 执行。GUI 交互、动效与视觉布局由用户验收。

TKCode.Arc 与 Avalonia 的许可见其上游仓库及 NuGet 包，项目许可见根目录 `LICENSE`。Windows 更新器和 macOS Sparkle 的许可随安装包携带。
