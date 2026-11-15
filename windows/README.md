# Codex Mixin Windows

Windows 使用 `desktop/CodexMixin` 中的 Avalonia + TKCode.Arc 共享桌面界面。Rust CLI 负责供应商、模型、Agent 接入、配置与网关生命周期；`windows/Core` 提供跨平台复用的严格 CLI contract adapter。

控制台包含模型与服务、Agent、模型路由、上下文、用量、设置六页。侧边栏、展开卡片、主题切换、表单、图表与路由画布使用 Arc 原生组件和动效。Ctrl+1..6 切页，Ctrl+R 刷新，Ctrl+B 收起侧边栏。

关闭窗口收起到原生托盘，再次运行应用激活已有实例。标题栏“更多 → 退出界面”结束 GUI；网关可在设置和托盘菜单中管理。未保存编辑在切页、刷新和退出前确认；操作失败保留输入。供应商密钥仅通过环境引用传给 CLI，不出现在进程参数或动作日志。

## 构建与打包

在仓库根目录使用 Rust stable、.NET 10 SDK 和 PowerShell：

```powershell
cargo build --locked --release
./windows/scripts/fetch-winsparkle.ps1
dotnet publish desktop/CodexMixin/CodexMixin.csproj -c Release -r win-x64 --self-contained true -o windows/build/publish
Copy-Item target/release/codex-mixin.exe windows/build/publish/
```

安装器和便携包沿用 `windows/scripts/package-windows.ps1`。发布文件仍名为 `codex_mixin_ui.exe` 和 `codex-mixin.exe`，保留已有 NSIS、注册表和快捷方式身份。完整包自带 .NET 运行时、WinSparkle 和许可信息，用户无需安装 .NET。

开发模式从仓库祖先目录查找构建后的 Rust CLI，发布模式使用同目录 CLI，不从 PATH 或当前目录加载任意程序。

## 更新与登录启动

WinSparkle 0.9.4 处理完整包下载和 Ed25519 验签。GUI 保留每小时检查、网关空闲 30 分钟后安装、草稿保护、版本一致性检查、退避重试及安装后恢复网关的流程。构建脚本验证固定版本归档，公钥与 macOS Sparkle 共用，发布仍生成安装版和便携版 appcast。

应用登录启动与 CLI 网关自启分别设置。应用启动项尊重 Windows 的禁用状态；轻量模式保留网关并退出 GUI，手动打开应用恢复界面。

## CI 与验收

`.github/workflows/windows.yml` 编译共享桌面包，并运行现有 Rust、CLI contract、安装与升级测试。微软 winapp 操作同次构建的真实 Arc 界面，使用真实 CLI 核对状态；失败保存脱敏窗口树、截图和日志，不跳过桌面失败。

本次迁移按用户要求不在本地运行测试。动效、布局、输入法、键盘导航、DPI 和无障碍交互由用户在真实平台验收。
