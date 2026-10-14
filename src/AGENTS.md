# Rust 源码规范

继承根规范。本目录容纳库核心与 CLI 二进制，不能把目录相邻当作允许任意互调。

- `lib.rs` 声明库模块和最小导出；`main.rs` 只组合 CLI 与 TUI。具体交互壳不能成为库依赖。
- 根级 `anthropic.rs` 是协议 DTO，`error.rs` 是错误边界，`gateway_access.rs` 是客户端访问凭据语义，`ducx_auth_carrier.rs` 是内部认证载体；新增逻辑按领域归属，不继续堆根级杂项工具。
- `config.rs`、`catalog.rs`、`ech.rs` 与同名目录共同组成一个模块；配置规则见 `config/AGENTS.md`，目录和 ECH 规则在本文件统一表达。
- 请求处理使用已组合的组件，不创建全局可变服务定位器。能力模块不引用 server、CLI 或具体 UI；OS 差异收敛到 platform。
- 跨模块类型默认限制在 crate 内；共享类型放在拥有该语义的模块，不借助重导出制造依赖环。
- catalog 保留模型来源、稳定 ID 和真实能力，目录缓存随配置/能力变化失效，不参与请求执行。
- benchmark 只报告真实计量，区分失败/取消与成功，限制并发、token 和费用；web_search 的未知/探测失败不能当作不支持，缓存按 provider 身份失效。
- images 解码限制尺寸、像素和体积，CPU 密集处理离开 runtime worker，相同输入保持稳定规范化结果；ECH 校验 DNS 二进制边界，保留既有显式降级与超时语义。
- 修改后执行根规范的 fmt、严格 Clippy、架构检查和 Rust 测试；协议、安全和 feature 路径按影响补充 E2E。
