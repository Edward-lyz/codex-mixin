# Codex 接入规范

继承上级规范。这里适配 Codex 安装、app-server、托管配置/认证、目录、历史与模式切换。

- 沿用 `bin`、`install`、`managed_config`、`managed_auth`、`catalog`、`history`、`switch`、`validate` 的职责；GUI 仅调用 CLI，不复制切换决策。
- official/mixin 切换必须保留原有配置与认证语义，拒绝会使托管 Codex 失去可用网关的非法停止。
- 只修改托管字段，保持用户非托管配置；写入使用既有原子文件能力，恢复时识别备份与用户修改冲突。
- Codex 可执行文件定位、安装目标和系统行为交给 platform；app-server 按真实协议验证，不凭进程存在宣称接入成功。
- history/认证内容按敏感数据处理；查询命令不顺带安装、修复或全局同步凭据。
- 运行 Codex 相关 Rust 测试；切换改动同时运行 `./scripts/test_codex_gateway_switch_state.sh`、`./scripts/test_application_lifecycle.sh`，确认 CLI 与 GUI 共用语义。
