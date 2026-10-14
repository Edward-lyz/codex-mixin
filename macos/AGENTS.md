# macOS GUI 壳规范

继承根规范。这里是 Swift/AppKit 菜单栏外壳，只消费真实 CLI contract，不实现网关/provider/客户端业务。

- View/Window 收集输入与展示状态，Actions/Support 组织 UI 操作，既有 runGateway/runGatewayStreaming 边界调用 CLI；不要把业务规则放进 AppDelegate 或 View。
- 使用 `--no-tui`、JSON 结果与结构化错误；路径/能力从 `interface --json` 获取。不得读写 CLI 状态文件来实现配置、备份、接入或服务管理。
- UI 状态在 MainActor 更新，子进程/网络/文件等待不阻塞主线程。stdout/stderr 同时收集，保持 JSON 通道干净，限制日志与缓冲开销。
- 生命周期、自启、official/mixin 切换由 CLI 决定；App 自身登录启动是壳职责。Sparkle 重启与明确退出行为不能混淆。
- 刷新任务使用现有 coordinator，避免重入和重复查询；窗口关闭/应用退出清理 timer、observer 与 UI task。操作超时不宣称回滚。
- 密钥以既有环境引用机制传给子进程，动作日志脱敏；壳自己的日志/展示缓存保存在原生目录。
- 新文案同步 en/zh-Hans；保持原生焦点、键盘导航、VoiceOver、浅深色和最低 macOS 13.1 兼容。不要为普通窗口新增独立 UI 框架。
- 执行对应 `./scripts/test_*.sh`、`./scripts/check_localizations.sh` 与 `./macos/build_app.sh`；图标变更补 `./scripts/verify_macos_app_icon.sh`。界面行为还需实际操作验证，静态源码测试不能替代。
