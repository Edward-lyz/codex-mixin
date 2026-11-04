# macOS GUI 壳规范

继承根规范。这里是 AppKit + SwiftUI 混合的 macOS 菜单栏外壳，只消费真实 CLI contract，不实现网关/provider/客户端业务。

## AppKit 与 SwiftUI 的边界

以组件需要的系统行为选择框架，不以“统一成纯 SwiftUI”或“清零 AppKit”为目标。架构调整必须保留既有视觉和交互；技术迁移不默认包含界面重设计，视觉变化应单独明确范围。

| 组件 | Mixin 的默认分工 |
| --- | --- |
| 菜单栏入口及标准菜单项 | AppKit 的 `NSStatusItem`、`NSMenu`、`NSMenuItem`；保留系统的勾选、禁用、高亮、快捷键、键盘导航及自动隐藏菜单栏行为 |
| 菜单内服务状态、额度等自定义内容 | SwiftUI 内容视图，通过局部 `NSHostingView` 嵌入；普通菜单操作不改成自绘按钮列表 |
| 控制台和现有次级窗口 | 现有 AppKit 窗口/控制器负责定位、焦点、关闭和草稿保护，内部页面使用 SwiftUI |
| 表单、卡片、列表、表格、图表 | 优先沿用 SwiftUI 及已有模型，不为更换框架重写业务或无依据地替换控件 |
| 确认框、文件选择、分享 | 按所在场景选择原生面板或 SwiftUI 声明式接口，保持模态关系、取消结果、默认按钮、焦点和权限语义 |
| 平台服务 | 应用生命周期、剪贴板、唤醒、登录启动、通知、更新器、图像资源等保持独立职责，不为框架纯度重写 |

- SwiftUI `Window`、`Settings`、`MenuBarExtra` 可以用于行为适配的场景，但逐个组件判断，不整应用统一替换。`MenuBarExtra(.window)` 的窗口式内容不视为 `NSMenu` 的等价替代。
- 不将系统确认框替换成普通独立窗口，也不因移除 AppKit 而取消未保存更改的保护、改变服务开关或凭据操作的确认语义。
- `Process`、`NSLock`、`NSCache`、`NotificationCenter` 属于 Foundation，不属于 AppKit 展示层。

## 桥接、状态与生命周期

- 桥接集中在菜单项或窗口容器边界。每个 `NSHostingView` / `NSHostingController` 明确唯一所有者、创建时机、重用范围和关闭后的处理，不让页面模型反向持有窗口或 Hosting 容器。
- 全局只保留真正共享的轻量状态；页面数据、草稿、订阅和任务按其作用域管理。不要为方便切换页面而让一个常驻全局模型永久持有所有页面。
- 菜单自定义行在展示前确定尺寸，随菜单宽度正确布局。释放或替换菜单容器时避开菜单事件回调的敏感时段，并处理快速关闭/重开。
- 关闭时区分可取消的读取与已经提交的业务操作；清理页面任务、订阅和临时资源，同时保留明确需要恢复的草稿。关闭窗口不等于业务回滚。
- 创建、重用、隐藏后保留、关闭后释放均须有明确理由。不得用空 `body` 冒充整个 Hosting 容器已释放，也不得声称 SwiftUI 场景关闭必然归还物理内存。
- 内存优化基于同口径的打开/关闭/重开和空闲/活跃测量，区分存活对象、分配器缓存、RSS 与物理占用；不以框架名称代替证据。

## 兼容性与界面验收

- 保持最低 macOS 13.1；较新 API 使用可用性分支。例如 `windowDismissBehavior` 仅在 macOS 15 及以上使用，旧系统不调用它，也不能因此丢失草稿。
- 界面改动检查菜单展开/重开、键盘导航、焦点、取消与确认、窗口关闭/重开、草稿恢复、浅深色和 VoiceOver。构建通过不等于这些行为已验证；未覆盖的平台或操作如实说明。
- 以下既有壳职责和安全规则继续适用，不因 UI 框架选择而改变。

## 壳职责与开发验证

- View/Window 收集输入与展示状态，Actions/Support 组织 UI 操作，既有 runGateway/runGatewayStreaming 边界调用 CLI；不要把业务规则放进 AppDelegate 或 View。
- 使用 `--no-tui`、JSON 结果与结构化错误；路径/能力从 `interface --json` 获取。不得读写 CLI 状态文件来实现配置、备份、接入或服务管理。
- UI 状态在 MainActor 更新，子进程/网络/文件等待不阻塞主线程。stdout/stderr 同时收集，保持 JSON 通道干净，限制日志与缓冲开销。
- 生命周期、自启、official/mixin 切换由 CLI 决定；App 自身登录启动是壳职责。Sparkle 重启与明确退出行为不能混淆。
- 刷新任务使用现有 coordinator，避免重入和重复查询；窗口关闭/应用退出清理 timer、observer 与 UI task。操作超时不宣称回滚。
- 密钥以既有环境引用机制传给子进程，动作日志脱敏；壳自己的日志/展示缓存保存在原生目录。
- 新文案同步 en/zh-Hans；保持原生焦点、键盘导航、VoiceOver、浅深色和最低 macOS 13.1 兼容。不要为普通窗口新增独立 UI 框架。
- 资源 manifest 与固定 Agent/provider ID 一致，保留来源/许可证；派生资源通过既有脚本更新。语言包 key、占位符和转义保持一致。
- 执行 `./scripts/check_localizations.sh` 与 `./macos/build_app.sh`；图标变更补 `./scripts/verify_macos_app_icon.sh`。界面行为通过人工操作验收，不写 Swift 测试。
