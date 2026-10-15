# Codex Mixin Windows 原生端

Windows 前端使用 **C# + XAML + WinUI 3 + Windows App SDK**。Flutter 工程、Runner、插件与补丁已移除。Rust CLI / 本地网关继续负责供应商、模型、客户端接入、配置事务和服务生命周期。

采用微软推荐的新建原生桌面应用路线（[开发路径](https://learn.microsoft.com/en-us/windows/apps/get-started/)、[WinUI 3](https://learn.microsoft.com/en-us/windows/apps/winui/)）。Windows App SDK 固定为 stable `1.8.260921001`，避免引入 preview / experimental API。

## 原生界面

- 与 macOS 对齐的信息架构：紧凑托盘主面板、模型与服务设置、Agent 接入管理、Fusion 与按时间轮转独立窗口。
- 模型与服务使用服务商侧栏、模型/连接设置页、Codex 模型选择表和底部统一「应用更改」。供应商图标与 macOS 共用来源。
- Fluent 原生控件、Mica 背景、系统明暗主题、高对比度、键盘导航与 UI Automation。
- 供应商新增、编辑、启停、删除、连接测试；模型搜索、选择、上下文覆盖、发现、能力探测和测速。
- OneAPI 认证桥接与上报、AWS 凭据、OpenCode Go 额度 Cookie 使用安全输入，经子进程环境传给 CLI。
- Agent 列表由 CLI 动态提供；支持版本检查、接入、断开和升级，Codex 接入必须明确选择模式。
- 用量时间范围、缓存命中、TTFT 与输出速率；失败展示原因，未知值不会显示为零。
- Fusion 编辑保留页面未编辑的已有字段，保存后由 CLI 应用配置。
- 原生托盘与菜单、关闭收起、单实例激活、每显示器 DPI；Explorer 重启后重新注册托盘。
- Windows 原生文件选择器；导入导出前说明 Base64 备份含凭据且不是加密。

托盘左键打开与 macOS 对齐的紧凑主面板，右键提供网关操作和窗口入口。所有窗口使用同一个 CLI adapter 和动作队列。外观选择作用于当前运行会话。GUI 登录启动保留已有 Run 项，新增安装默认由用户在设置中启用；不会覆盖系统禁用状态，也不在登录路径启动网关。

## 工程结构与环境

| 路径 | 职责 |
| --- | --- |
| `CodexMixin/` | WinUI 原生窗口、页面与 Windows 平台壳 |
| `Core/` | CLI 进程、严格 JSON 契约与保存后应用配置 |
| `Tests/` | 子进程边界、凭据、超时、取消、结构化错误与真实 CLI 契约 |
| `UiTests/` | Windows UI Automation 导航、主题、托盘恢复与截图 |
| `scripts/` | 图标生成、自包含发布、NSIS、安装与卸载 |

开发要求 Windows 10 1809 或更高版本、.NET 10 SDK、Visual Studio 的 WinUI 开发工具和 Windows SDK，以及 Rust 工具链。建议使用当前受支持的 Windows 11。WinUI 的 XAML 编译器和 UI 测试必须在 Windows 执行。

```powershell
cargo build --locked --release
./windows/scripts/generate-windows-assets.ps1
dotnet build windows/CodexMixin/CodexMixin.csproj -c Debug -p:Platform=x64
```

开发模式在项目祖先目录查找 `target/release/codex-mixin.exe` 或 `target/debug/codex-mixin.exe`，发布模式优先调用同目录 CLI。不会从 PATH 或当前工作目录加载任意 CLI。

## CI 验证与发布

`.github/workflows/windows.yml` 在 GitHub Windows runner 运行 Rust 检查与测试、C# 适配层及真实 CLI 契约测试、自包含 WinUI 发布、Windows UI Automation 测试，最后生成安装器和便携包。本次迁移按用户要求不在本机运行测试。

每次成功的 UI 测试都截图托盘主面板、模型与服务、连接设置、Agent、Fusion 两种模式与设置的浅色和深色版本，并保存首次启动、空状态、停止状态与窄窗口截图。供应商与 Fusion 的已配置画面使用经真实 CLI 导入的隔离 CI fixture，凭据为虚构值，不连接外部模型服务。失败时也尝试保存现场截图与错误文本。

在 GitHub Actions 的 Windows run 页面下载 `windows-native-ui-evidence` artifact，解压后在 macOS 浏览器打开 `index.html`，即可查看全部页面；点击缩略图可查看原始 PNG。artifact 保留 14 天，仅包含截图、图集与错误文本，不上传配置目录。

一键打包仍从仓库根执行：

```powershell
powershell -ExecutionPolicy Bypass -File windows/scripts/package-windows.ps1
```

输出 `dist/codex-mixin-windows/`、`dist/CodexMixin-Setup.exe`、`dist/CodexMixin-Windows.zip`。保留 `codex_mixin_ui.exe` 与 `codex-mixin.exe` 的原文件名、NSIS 注册表身份和快捷方式路径；应用目录包含 .NET 与 Windows App SDK runtime，用户无需安装 Flutter 或 .NET。

安装/卸载继续保留用户配置；未发布 UI 单独可执行文件，必须使用完整应用目录。签名时覆盖随包发送的 EXE / DLL 与安装器。发布工作流沿用相同打包脚本。
