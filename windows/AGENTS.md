# Windows GUI 壳规范

继承根规范。这里是 Flutter 桌面外壳，只消费真实 Rust CLI contract；业务策略由 CLI/core 负责。

- UI 输入与状态留在 Dart，CLI 调用集中在既有 `lib/cli.dart`/controller 边界；不得复制 provider 校验、接入、模型路由或服务生命周期规则。
- 使用 `--no-tui`、JSON 和结构化错误；路径/能力由 interface 返回。配置备份也调用 CLI，不猜测或直写 CLI 状态目录。
- 密钥用环境引用传递，禁止出现在 argv/动作日志；壳自己的日志/缓存放原生目录。
- App 的托盘/窗口/登录启动与 CLI 网关自启分开；查询未知状态不能展示为已关闭或已是最新。
- 保持 `windows/windows/` 的 Flutter 工程结构；原生 runner 只处理窗口嵌入，生成文件通过工具更新。
- Widget 经 controller/cli 访问业务，JSON 在适配边界校验；async 更新遵守 mounted/dispose 生命周期，缺字段或解析失败不返回空成功。
- 安装/卸载保留用户配置，PATH 只修改本项目项；插件 patch 匹配已知版本，不匹配即失败。资源、fixture 和生成子目录继承本文件，不另设规范。
- 在 `windows/` 工作目录运行 `flutter analyze`、`flutter test`；资源先运行 `powershell -File scripts/generate-windows-assets.ps1`。
- 构建/原生变更在 Windows 执行 `flutter build windows --release`；打包在仓库根执行 `powershell -ExecutionPolicy Bypass -File windows/scripts/package-windows.ps1`。非 Windows 主机明确报告未验证项。
