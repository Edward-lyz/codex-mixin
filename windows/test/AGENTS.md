# Flutter 测试规范

继承上级规范。这里验证 Widget、状态呈现、CLI contract 消费与资源映射。

- 使用明确成功/失败/unknown/committed fixture，不把 fake CLI 结果当作真实后端成功证据。
- Widget 测试驱动实际点击、输入、busy 状态与错误展示，不能只断言组件存在。
- contract fixture 与 Rust 输出一致；新增字段/错误码同时核对两侧，不用宽松动态值掩盖解析失败。
- 图标测试验证固定 Agent ID、manifest 与真实资源引用；测试前运行 Windows 资源生成入口。
- 隔离真实进程、用户配置和托盘；CLI 实际可用性由 Rust/集成验证承担。
- 在 `windows/` 执行 `flutter test`、`flutter analyze`；修 bug 先观察复现失败，再修复并观察通过。
