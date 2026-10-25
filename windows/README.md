# Codex Mixin Windows 原生端

Windows 前端使用 **C# + XAML + WinUI 3 + Windows App SDK**。Flutter 工程、Runner、插件与补丁已移除。Rust CLI / 本地网关继续负责供应商、模型、客户端接入、配置事务和服务生命周期。

采用微软推荐的新建原生桌面应用路线（[开发路径](https://learn.microsoft.com/en-us/windows/apps/get-started/)、[WinUI 3](https://learn.microsoft.com/en-us/windows/apps/winui/)）。Windows App SDK WinUI 模块固定为 stable `1.8.260803003`，Foundation、InteractiveExperiences 和 Base 沿用 lock file 中同一发行组的版本，避免引入 preview / experimental API。

## 原生界面

- 紧凑托盘主面板；控制台顶部依次切换模型与服务、Agent 接入、用量明细、上下文，Fusion 与按时间轮转使用独立窗口。
- 模型与服务使用服务商侧栏、模型/连接设置页、Codex 模型选择表和底部统一「应用更改」。供应商图标与 macOS 共用来源。
- Fluent 原生控件、Mica 背景、系统明暗主题、高对比度、键盘导航与 UI Automation。
- 供应商新增、编辑、启停、删除、连接测试；模型搜索、选择、上下文覆盖、发现、能力探测和测速。
- OneAPI 认证桥接与上报、AWS 凭据、OpenCode Go 额度 Cookie 使用安全输入，经子进程环境传给 CLI。
- Agent 列表由 CLI 动态提供；支持版本检查、接入、断开和升级，Codex 接入必须明确选择模式。
- 用量时间范围、缓存命中、TTFT 与输出速率；失败展示原因，未知值不会显示为零。
- Fusion 编辑保留页面未编辑的已有字段，保存后由 CLI 应用配置。
- 上下文页把引擎开关、阈值与按会话统计放在一起；保存只提交改动过的字段，CLI 自动重启运行中的网关一次使设置生效，无改动不重启，已停止的网关保持停止。没有缓存记录或更新时间时显示未知而不是零。
- 原生托盘与菜单、关闭收起、单实例激活、每显示器 DPI；Explorer 重启后重新注册托盘。
- AGY 与 WorkBuddy 的 Windows 图标使用原 SVG 的 128px PNG 派生资源，保留滤镜效果；源 SVG 仍供 macOS 使用。AGY 模型目录解析上游 `maxTokens` 与 `maxOutputTokens`，模型上下文缺失时显示未知，不冒充默认值。
- Windows 原生文件选择器；导入导出前说明 Base64 备份含凭据且不是加密。

托盘左键打开与 macOS 对齐的紧凑主面板，右键提供网关操作和窗口入口。所有窗口使用同一个 CLI adapter 和串行动作队列；只读查询独立限流，本地快照最多并发两个 CLI 子进程，远程额度另限一个，不阻塞导航和本地状态。外观选择作用于当前运行会话。GUI 登录启动保留已有 Run 项，新增安装默认由用户在设置中启用；不会覆盖系统禁用状态，也不在登录路径启动网关。

托盘重开时先展示上次额度，状态行标记正在刷新；尚无成功数据时显示加载或未知，刷新失败保留上次成功数据。服务状态、供应商列表和额度独立查询，供应商切换使用同一次额度快照，不再重复发请求。供应商侧栏也按可见行创建控件。点击其他窗口或桌面可收起正在查询的托盘面板。切换控制台页签和重新激活窗口保留最大化，仅最小化窗口需要还原。

托盘首次显示即使用紧凑高度，布局完成后只测量内容与菜单，避免重新测量窗口根节点。状态返回后立即启用网关开关，不等待网络额度；额度失败保留已知服务状态。本地读取最多等待 10 秒，额度最多 20 秒，期限包含查询队列等待；超时显示错误。收起面板保留后台预取，主动刷新、操作网关或退出会取消过期读取并回收子进程，配置操作仍执行完毕。

应用启动时先载入 GUI 自有目录中的额度展示缓存，再预取所有已启用服务商；与 macOS 一样每 10 秒尝试后台刷新，在途请求合并。缓存只保存图标、名称、额度数字和更新时间，不保存凭据或原始上游响应，不访问 CLI 配置目录。切换服务商直接显示其快照；「刷新额度」按钮可立即重新读取。每个服务商结果独立更新，远程失败保留数字和时间，不将旧数据冒充新结果。Windows 11 的托盘窗口关闭 DWM 外框描边，面板自身也不重复绘制白边。

控制台首次打开使用 1440×960 逻辑尺寸，按显示器 DPI 缩放并限制在工作区的 92% 内，居中显示。用量明细只显示请求与 Token 等网关统计，不查询或展示供应商额度。圆环百分比使用窗口主题资源，浅色、深色和运行中主题切换均跟随文字颜色。Windows 应用、标题栏和托盘图标从 macOS 原图移除边缘连通的中性背景，保留中间白色终端符号，生成透明 PNG 与多分辨率 ICO。

CLI 进程启动和 JSON 解析在后台线程执行。未显式配置 `TOKIO_WORKER_THREADS` 时，仅查询子进程使用 2 个 Tokio worker，避免每次读取都为所有逻辑 CPU 创建线程；变更与守护进程沿用原环境。隐藏或最小化控制台暂停刷新定时器和加载动画；设置、Fusion、Agent 和用量页面不启动无用的轮询，离开模型页面释放编辑器引用，固定图标复用解码资源。模型行离开虚拟化回收池时移除测速控件引用，避免滚动一次就永久持有整行。安装器使用 LZMA solid 压缩，分发目录不含 PDB；只引用 WinUI 模块及其依赖，使用 SDK 官方的组件自包含载荷，避免 Runtime 总包的完整 MSIX 把未使用的 AI、ML、Widgets、DWrite 和 Tensors 再次打入分发目录；保留自包含运行时，不启用会影响 XAML 反射的 trimming。

用量明细使用固定视口的虚拟列表，只为可见行创建控件；Fusion 模型选择同样使用原生虚拟列表，筛选不会丢失隐藏项的选择。会话筛选只更新摘要和明细，图表分组只更新图表。图表先按日期、供应商聚合，再绘制柱形，复杂度从每个日期/系列反复扫描记录，降为 `O(N + P log P + D×P)`；差分用例核对聚合/拆分、相同排序权重、窗口外数据和空数据的结果。

供应商配置和 Agent 检测快照最多复用 30 秒，页面显示读取时间；显式刷新或应用内配置操作使快照失效。CLI 安装检测仍由核心执行，未检测到与检测失败不会被替换为成功。

返回模型页先渲染近期配置与已有测速结果，再独立刷新测速，不等待额外 IPC 才恢复页面交互。

本地用量并发读取，网络额度独立加载，不锁住整页操作。新导航可以取消已经过期的页面读取，配置写入仍会执行完毕；页面离开或关闭会取消其额度请求。`--background` 只创建托盘所属窗口，不显示并加载完整模型页，首次打开时才读取 CLI。自定义标题栏为页签设置指针透传区域，不支持自定义标题栏的系统仍保留正常内容区中的页签。

## 工程结构与环境

| 路径 | 职责 |
| --- | --- |
| `CodexMixin/` | WinUI 原生窗口、页面与 Windows 平台壳 |
| `Core/` | CLI 进程、严格 JSON 契约与保存后应用配置 |
| `Tests/` | 子进程边界、凭据、超时、取消、结构化错误与真实 CLI 契约 |
| `UiTests/` | Windows UI Automation 导航、主题、托盘恢复与截图 |
| `scripts/` | 图标生成、自包含发布、NSIS、安装与卸载 |

开发要求 Windows 10 1809 或更高版本、.NET 10 SDK、Visual Studio 的 WinUI 开发工具和 Windows SDK，以及 Rust 工具链。建议使用当前受支持的 Windows 11。WinUI 的 XAML 编译器和 UI 测试必须在 Windows 执行。

命令行编译可使用 Visual Studio 2022 Build Tools 的「使用 C++ 的桌面开发」工作负载，包含 MSVC x64/x86、Windows SDK 和 CMake。仅安装 .NET 运行时不能编译界面；仅安装 Rust 也缺少链接所需的 `kernel32.lib`。安装完成后重新打开终端，在 x64 Developer PowerShell 中构建 CLI，再使用 .NET SDK 构建界面。

```powershell
cargo build --locked --release
./windows/scripts/generate-windows-assets.ps1
dotnet build windows/CodexMixin/CodexMixin.csproj -c Debug -p:Platform=x64
```

开发模式在项目祖先目录查找 `target/release/codex-mixin.exe` 或 `target/debug/codex-mixin.exe`，发布模式优先调用同目录 CLI。不会从 PATH 或当前工作目录加载任意 CLI。

## CI 验证与发布

`.github/workflows/windows.yml` 在 GitHub Windows runner 运行 Rust 检查与测试、C# 适配层及真实 CLI 契约测试、自包含 WinUI 发布、Windows UI Automation 测试，最后生成安装器和便携包。本次迁移按用户要求不在本机运行测试。

每次成功的 UI 测试都截图托盘主面板、模型与服务、连接设置、Agent、上下文、Fusion 两种模式与设置的浅色和深色版本，并保存首次启动、空状态、停止状态与窄窗口截图。供应商与 Fusion 的已配置画面使用经真实 CLI 导入的隔离 CI fixture，凭据为虚构值，不连接外部模型服务。失败时也尝试保存现场截图与错误文本。

在 GitHub Actions 的 Windows run 页面下载 `windows-native-ui-evidence` artifact，解压后在 macOS 浏览器打开 `index.html`，即可查看全部页面；点击缩略图可查看原始 PNG。artifact 保留 14 天，包含截图、图集、错误文本和 `resources.json`，不上传配置目录。规模回归使用真实网关记录 500 条回复，并由 CLI 导入 600 个模型，检查用量、Fusion 和滚动模型表的控件数随可见行而非总数据增长。资源记录包含显示/隐藏时私有内存、隐藏时 working set，以及超过刷新周期的 35 秒空闲 CPU 采样；导航耗时包含 UI Automation 调用与 200 ms 轮询开销，用于同类 runner 比较，不等同于输入到呈现帧的延迟。它是单次 runner 数据，不代表用户硬件上的能耗或与 macOS 的对比结论。

一键打包仍从仓库根执行：

```powershell
powershell -ExecutionPolicy Bypass -File windows/scripts/package-windows.ps1
```

输出 `dist/codex-mixin-windows/`、`dist/CodexMixin-Setup.exe`、`dist/CodexMixin-Windows.zip`，并在安装器旁保存 `.metrics.json`，记录展开目录、安装器和便携包的字节数。保留 `codex_mixin_ui.exe` 与 `codex-mixin.exe` 的原文件名、NSIS 注册表身份和快捷方式路径；应用目录包含 .NET 与 Windows App SDK runtime，用户无需安装 Flutter 或 .NET。

安装/卸载继续保留用户配置；未发布 UI 单独可执行文件，必须使用完整应用目录。签名时覆盖随包发送的 EXE / DLL 与安装器。发布工作流沿用相同打包脚本。
