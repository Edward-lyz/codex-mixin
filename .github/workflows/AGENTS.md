# CI 与 Release 工作流规范

继承上级规范。`ci.yml` 验证 Linux/macOS，`windows.yml` 验证 Windows，`release.yml` 处理发布。

- Rust 门禁保留 fmt、严格 Clippy、架构正负例、默认与 `client-auth` 测试，以及缓存 E2E；平台 GUI/打包保留对应 runner。
- 命令注明 working-directory，缓存按输入有效性使用；依赖工具缺失立即失败，不跳过关键检查。
- 条件表达式覆盖 PR/push/manual/release 场景，fork PR 不需要写权限或发布 secrets。
- Artifact 名称、架构、版本和产物路径一致，缺产物 fail fast；签名/发布凭据不进入日志。
- release 与验证保持不同权限与触发条件；更新 action/toolchain 说明兼容影响。
- 静态检查 YAML 和本地命令/路径，实际 CI 成功需读取对应 run；本地 lint 不等于 Windows/macOS 打包通过。
