# E2E 测试规范

继承根规范。这里是仓库唯一的 Rust 测试目录，通过真实 CLI、HTTP/WS 与存储路径验证端到端的正常行为。

- 驱动真实 Router/CLI/存储路径，本地受控 provider 可用于确定性测试；不把 fixture/mock 成功报告成线上验证。
- 新增或删除顶层 `tests/*.rs` 时同步 `windows_integration_shards.txt`；改变 `client-auth` 行为的用例登记在 `client_auth_tests.txt`。
- 运行真实二进制的测试必须经 `tests/common` 的 `cli`/`tokio_cli` 构造 Command：它把 HOME、CODEX_HOME、配置/runtime/daemon 文件、XDG 目录指向临时目录，并设置唯一 `CODEX_GATEWAY_SERVICE_ID`，使 launchd/systemd/任务计划服务身份与用户真实网关隔离；直接使用 `CARGO_BIN_EXE_*` 会被 `check_arch.sh` 拒绝。
- 每个测试使用临时配置/HOME、随机端口与受控进程；并发执行不能共享真实状态或修改全局环境造成串扰。
- 安全断言覆盖认证隔离、内部错误脱敏、请求大小、配置保留和平台边界，不能为通过而删减。
- 状态变更同时断言持久化与失败阶段；流式测试验证事件/取消/完成语义，不只看 HTTP 200。
- `real_models.rs` 等需要真实凭据/付费请求的测试保持显式 ignored；未运行时明确报告，不能算入线上成功。
- fixture 保持最小、确定性、脱敏，说明来源与生成方式；真实二进制样例不得用占位文本替换，离线通过不能宣称线上验证成功。
- 运行受影响的 `cargo test --locked --test <name>`，Rust 变更最终执行根级全目标门禁；fixture 子目录继承本文件。
