# Rust 源码规范

继承根规范。本目录容纳库核心与 CLI 二进制，不能把目录相邻当作允许任意互调。

- `lib.rs` 声明库模块和最小导出；`main.rs` 只组合 CLI 与 TUI。具体交互壳不能成为库依赖。
- 根级 `anthropic.rs` 是协议 DTO，`error.rs` 是错误边界，`gateway_access.rs` 是客户端访问凭据语义，`ducx_auth_carrier.rs` 是内部认证载体；新增逻辑按领域归属，不继续堆根级杂项工具。
- `config.rs`、`catalog.rs`、`ech.rs` 与同名目录共同组成一个模块。修改这些根文件时同时读取对应目录规范；不能因文件在目录外而跳过其契约。
- 请求处理使用已组合的组件，不创建全局可变服务定位器。能力模块不引用 server、CLI 或具体 UI；OS 差异收敛到 platform。
- 跨模块类型默认限制在 crate 内；共享类型放在拥有该语义的模块，不借助重导出制造依赖环。
- 修改后执行根规范的 fmt、严格 Clippy、架构检查和 Rust 测试；协议、安全和 feature 路径按影响补充 E2E。
