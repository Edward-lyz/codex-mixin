# 网关服务执行规范

继承上级规范。本目录与 `../service.rs` 共同实现网关生命周期入口，分离 daemon 与 logging。

- 生命周期策略复用 application，OS 服务执行复用 platform；启动/停止不得改变用户的 autostart 选择。
- service ensure 按当前版本、服务定义和实际状态收敛；PID/元数据过期不能误杀无关进程。
- 就绪必须来自真实绑定/健康检查，超时保留原因。GUI 等待超时不表示网关已停止或配置已回滚。
- daemon 退出时清理其拥有的元数据、任务与子进程；日志轮转和文件权限沿用现有实现。
- 查询 JSON 保持与 macOS/Windows 壳一致；日志不污染 stdout，重启不重复创建孤立服务。
- 验证重复启动、版本替换、自启保持、停止与陈旧元数据，运行 `cargo test --locked --test gateway_startup`、CLI runtime 测试及根级门禁。
