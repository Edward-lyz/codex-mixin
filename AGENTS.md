# Codex Mixin 项目守则

Codex Mixin 是一个 Rust 本地网关 + CLI + macOS 菜单栏 App，把 OpenAI Chat Completions / Anthropic Messages 兼容 provider 接入官方 Codex。

**Rust**
- `unsafe_code = "forbid"`，不要引入 unsafe。
- 生产路径不用 `.unwrap()` / `.expect()` / `panic!`；用 `?` 传播，跨层用 `anyhow::Context::context` 补语义。`unwrap` 只允许出现在 `#[cfg(test)]` 内。
- HTTP 边界错误走 `GatewayError`（`src/error.rs`，thiserror + `IntoResponse`），内部逻辑返回 `anyhow::Result`。`Io` 和 `Other` 对客户端只回 `internal server error`，绝不把内部细节、路径、拓扑写进响应体。
- 日志用 `tracing`：客户端错误 `warn!`，上游/内部错误 `error!`，落日志时用 `format_error_chain` 打完整链。库代码不要 `println!`。
- 沿用现有依赖（anyhow、thiserror、serde、tokio、reqwest、axum、bytes），默认不新增 crate。命名和模块划分沿用相邻代码，源码保持 ASCII。
- 建议本地跑 `cargo clippy --all-targets` 保持无 warning（CI 只强制 fmt + test）。
- 编译完后，及时 `cargo clean` 清理空间

**测试**
- 单测就近放同文件的 `#[cfg(test)] mod tests`，或模块目录下的 `tests.rs`；跨组件集成测试放 `tests/`。
- 提交前必过 CI 三项：`cargo fmt --all -- --check`、`cargo test --locked --all-targets`、涉及 prompt cache 时 `./scripts/e2e_prompt_cache.sh`。`Cargo.lock` 已提交，测试带 `--locked`。
- macOS 相关改动跑 `./macos/build_app.sh` 和对应 `./scripts/test_*.sh`。
- 测真实行为，不为过测试而 fake、mock 或放宽断言。安全类断言（如 `internal_errors_do_not_expose_details`）不能删。

**功能规范**
- fail fast，禁止 hidden fallback：不静默降级、不返回 mock/默认值、缺 key 或配置时不跳过逻辑。
- 保持协议保真：OpenAI Chat Completions 与 Anthropic Messages 双格式、SSE 流式、prompt-cache 前缀形状都要正确转换，别破坏 provider 语义。
- 绝不提交或打印 API Key / Token / Cookie，密钥只在既有存储路径流转。
- 一次改动只做一件事，不夹带无关修改。

**性能**
- 网关转发是热路径：流式透传 SSE，别把整个响应体 buffer 进内存，避免多余 clone 和分配，复用现有 `bytes` / `memchr` 模式。
- async 路径里不做阻塞 IO，别卡住 tokio runtime。
- release 的 `lto = "thin"` 和 `codegen-units = 1` 保持不动。
- 引入任何改动前都需要考虑内存消耗和 CPU 消耗，我们目的最低硬件是树莓派级别
