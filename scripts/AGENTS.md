# 验证、构建与打包脚本规范

继承根规范。这里提供可复现工程入口，不承载 GUI 私有业务或替代 Rust 核心。

- Bash 沿用 `set -euo pipefail`、ROOT 定位、引用路径和 trap 清理；外部命令检查退出码，保留失败阶段。
- 临时目录与端口隔离，清理仅限脚本拥有的已确认路径；不要用通配符删除用户数据。
- 运行 `codex-mixin` 的脚本必须在临时目录内设置 HOME、CODEX_HOME、配置/runtime/daemon 文件和 `CODEX_GATEWAY_SERVICE_ID`，并在 trap 中只清理这些隔离实例，不触碰真实网关与系统服务。
- test 脚本运行实际测试入口；源码匹配只能检查结构约束，不能冒充运行时/UI 验证。
- 架构扫描是粗粒度门禁，变更规则补负例；不通过重命名/别名/放宽断言绕过边界。
- macOS 非 UI Swift 测试统一登记在 `test_macos.sh` 的表格中（名称、localization、flags、frameworks、sources），新增测试加一行，不再新增单独脚本。
- E2E 使用受控本地 provider；需要真实凭据/网络的入口说明前提与成本，不在日志输出密钥。
- 下载/打包使用既有版本锁、签名/校验机制，文件缺失立即失败；dist/target/第三方缓存不提交。
- Bash 改动用 `bash -n <script>`，Node 用 `node --check <script>`，Python 按上级规范执行静态检查；再实际运行受影响入口，长日志重定向并报告退出码。
- 只改规范时运行 `./scripts/check_arch.sh`、`./scripts/test_check_arch.sh`、继承/引用检查与 `git diff --check`，不无条件重编译所有平台。
