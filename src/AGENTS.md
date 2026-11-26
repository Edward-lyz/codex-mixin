# Rust 源码规范

继承根规范。本目录容纳库核心与 CLI 二进制，不能把目录相邻当作允许任意互调。

- `lib.rs` 声明库模块和最小导出；`main.rs` 只组合 CLI 与 Web 面板。具体交互壳不能成为库依赖。
- 库代码按 MIT 公理设计严格分为 4 层（`core/` ← `infra/` ← `runtime/` ← `app/`），依赖方向严格下三角（由 `scripts/check_arch.sh` 强制校验 4 层宏边界 + 19 模块 Tarjan SCC 零环）：
  - `core/`：`access.rs`、`error.rs`、`usage.rs`、`images/`、`protocol/`（含 `anthropic.rs`、`types.rs`）、`provider/`（含 `baidu/auth_carrier.rs`）
  - `infra/`：`config/`、`clients/`、`ech/`、`pi_plugins/`、`web_search/`
  - `runtime/`：`catalog/`、`context/`、`upstream/`、`gateway/`、`fusion/`、`benchmark/`、`server/`
  - `app/`：离线管理用例编排（原 `application/`）
  - `cli/`：二进制交互壳；根目录仅保留外壳基础设施，所有子命令统一放在 `cli/commands/`（`connect/`、`settings/`、`providers/`、`service/`、`doctor/`、`status/` 等）。
- `lib.rs` 通过根级重导出保持 `codex_mixin::*` 兼容路径；所有多文件模块统一采用 `mod.rs` 目录组织，不再混用 `foo.rs` + `foo/`。
- 请求处理使用已组合的组件，不创建全局可变服务定位器。能力模块不引用 server、CLI 或具体 UI；OS 差异收敛到 platform。
- 跨模块类型默认限制在 crate 内；共享类型放在拥有该语义的模块，不借助重导出制造依赖环。
- catalog 保留模型来源、稳定 ID 和真实能力，目录缓存随配置/能力变化失效，不参与请求执行。
- benchmark 只报告真实计量，区分失败/取消与成功，限制并发、token 和费用；web_search 的未知/探测失败不能当作不支持，缓存按 provider 身份失效。
- images 解码限制尺寸、像素和体积，CPU 密集处理离开 runtime worker，相同输入保持稳定规范化结果；ECH 校验 DNS 二进制边界，保留既有显式降级与超时语义。
- 修改后执行根规范的 fmt、严格 Clippy、架构检查和 Rust 测试；协议、安全和 feature 路径按影响补充 E2E。
