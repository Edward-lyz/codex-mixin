# 自适应应用图标规范

继承上级规范。这里是 Apple 自适应图标源包，`icon.json` 与 `Assets/` 共同交付。

- 保留工具要求的包结构、资源名和元数据；图层与浅深色资源按现有约定更新。
- 这里与传统 icns、Info.plist 和 Windows 派生 ico 保持产品标识一致；生成产物通过既有脚本更新。
- 不直接修改 dist App 内的 Assets.car 来冒充源包修复。
- 图标设计改动保留透明度与不同外观下的轮廓可辨识度，实际检查系统菜单/窗口用途。
- 执行 `./scripts/test_application_icon.sh`、`./scripts/verify_macos_app_icon.sh` 及 App 构建；缺适配工具时报告具体限制。
