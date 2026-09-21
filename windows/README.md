# Codex Mixin Windows 端

Codex Mixin 的 Windows 桌面界面，使用 **Flutter** 构建，是 Rust 本地网关（`codex-mixin`）在 Windows 上的图形控制端。它与 `macos/`、`tui/` 一样是独立壳层，只消费 CLI JSON contract。

界面本身不实现任何网关逻辑：它定位并调用真实的 `codex-mixin.exe`（优先用已安装包中的可执行文件，其次用仓库 `target/release/`），完成供应商管理、网关启停、模型选择与测速、配置备份，以及 Codex、Claude Code、DSH、OpenCode、Pi 的接入与恢复。供应商图标复用 `macos/assets/providers` 下的 SVG，通过 `flutter_svg` 渲染。

DUCX `.tar.bz2` 由 Rust core 直接解压，不依赖或捆绑第三方归档程序。

## 目录结构

- `lib/` — Dart 前端代码（UI 主体，日常改动都在这里）。
  - `main.dart` 入口、`tray_page.dart` 托盘弹窗、`settings_page.dart` 供应商设置、`benchmark_page.dart` 测速、`fusion_page.dart`/`flyout_page.dart` 等页面，`controller.dart` 调用 CLI，`theme.dart` 主题，`widgets.dart` 通用组件。
- `windows/` — **Flutter 自动生成的原生 Windows Runner（C++ 嵌入层）**。包含 `runner/`、`flutter/`、`CMakeLists.txt`。
  - 注意：这个子目录名由 Flutter 工具链强制约定，`flutter build windows` 只认 `windows/` 这个名字，**不能改名**（改成 `win` 等会导致构建失败）。若要去掉“windows/windows”的双层重名，只能重命名外层的 Flutter 工程目录（即本目录 `windows/`），并同步更新 CI 与打包脚本中的路径。
- `scripts/` — Windows 专用的打包、安装、修补脚本（详见下文），镜像 macOS 把构建脚本放在 `macos/` 下的做法。
- `test/` — Flutter widget 测试（对应 `macos/tests/` 的 Swift 测试）。
- `assets/` — 供应商图标与托盘图标：**由 `scripts/generate-windows-assets.ps1` 从 `macos/` 生成，不入库（已 gitignore）**。
- `build/` — Flutter 构建产物（自动生成，勿提交）。

## 环境要求

- Flutter（stable 通道）与 Visual Studio 的“使用 C++ 的桌面开发”工作负载。
- Rust 工具链（用于构建被界面调用的 `codex-mixin.exe`）。

## 开发与运行

在本目录（`windows/`）下执行：

```powershell
flutter pub get
powershell -File scripts/generate-windows-assets.ps1
flutter run -d windows
```

> 供应商 SVG 与 `.ico` 图标不入库（唯一来源在 `macos/`），首次或清检出后必须先跑 `scripts/generate-windows-assets.ps1` 生成，否则 `flutter build/run` 会因缺资源失败。一键打包脚本 `package-windows.ps1` 会自动执行这一步。

## 静态检查与测试

```powershell
flutter analyze
flutter test
```

## 构建 Release UI

```powershell
flutter build windows --release
```

产物位于 `windows/build/windows/x64/runner/Release/`。

## 一键打包（推荐）

打包脚本会依次构建 Rust CLI 与 Flutter UI、组装应用目录、生成 NSIS 安装器。在仓库根目录执行：

```powershell
powershell -ExecutionPolicy Bypass -File windows/scripts/package-windows.ps1
```

产物：
- `dist/codex-mixin-windows/` — 组装好的应用目录（含 `codex-mixin.exe` 与 `codex_mixin_ui.exe`）。
- `dist/CodexMixin-Setup.exe` — 标准 Windows 安装器。

可选参数：`-Sign` 开启签名、`-Version x.x.x.x` 指定安装器版本、`-NsisPath` 指定 `makensis.exe`。

## windows/scripts 脚本说明

- `package-windows.ps1` — 打包主入口（构建 + 组装 + 生成安装器）。
- `installer.nsi` — NSIS 安装器脚本，安装到 `%ProgramFiles%\Codex Mixin`（需管理员权限）；安装前会依据卸载注册表项里记录的位置，先卸载任意旧版本，再创建开始菜单快捷方式与卸载项。
- `install-windows.ps1` / `uninstall-windows.ps1` — 免安装器的手动安装/卸载脚本；安装时把安装目录加入用户 PATH、并对 `~/.codex-mixin` 配置目录做一次性 ACL 加固，卸载时清理 PATH 与快捷方式。
- `generate-windows-assets.ps1` — 从 `macos/CodexMixin.icon/Assets/CodexMixin.png` 生成 `.ico` 图标，并从 `macos/assets/providers/` 同步供应商 SVG 到 `windows/assets/providers/`；这些产物均不入库，由本脚本按需生成。
- `patch-window-manager.ps1` / `patch-multi-window-rounding.ps1` — 在 `flutter pub get` 后修补 `window_manager` / `desktop_multi_window` 插件的原生代码（窗口行为/圆角），需在 `flutter build` 前执行。

## 托盘行为

Release 版会创建系统托盘图标：左键点击弹出紧凑面板，点击面板外部自动隐藏；完整的“供应商设置”窗口与托盘面板可同时打开。面板提供与 macOS 一致的二级菜单：模型与服务、Fusion、安装/恢复（Codex、Claude Code、DSH、OpenCode、Pi），以及关于/日志/配置目录等操作。

## 与 Rust 网关的关系

- 界面所有实际操作都通过命令行调用 `codex-mixin.exe`，UI 触发的动作输出会落盘到运行日志，便于排查。

## 持续集成

`.github/workflows/windows.yml` 在 `windows-latest` 上分别执行 Rust 检查/测试/构建、Flutter 分析/测试/构建，以及 `windows/scripts/package-windows.ps1` 打包并上传 `dist` 产物。
