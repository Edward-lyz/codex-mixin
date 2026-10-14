# 登录自启服务规范

继承上级规范。本目录与 `../startup_service.rs` 适配 launchd、systemd user unit 与 Windows 当前用户任务。

- 服务定义存在即表示用户开启网关自启；start/stop/restart、升级和定义迁移不改变此选择。
- `definition` 生成明确、可验证的服务内容，各 OS 文件只执行对应管理器操作；unsupported 明确报告不支持。
- 区分 installed、current、loaded，不凭单一 PID 推断所有状态。
- 作用范围保持当前用户；服务参数、工作路径和日志路径正确转义，避免 shell 拼接与路径截断。
- 这里的 OS 工具调用会阻塞，async 调用方必须隔离；错误保留工具退出码和脱敏原因。
- 定义渲染在所有主机测试，真实启停由对应 OS 验证；覆盖用户自启选择在重启/升级后保持不变。
