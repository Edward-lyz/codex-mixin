# 扩展指南

如何在不破坏架构边界的前提下扩展 codex-mixin。

## 新增一个 provider 协议

1. 在 `provider` 定义协议枚举与路由元数据（纯规则）。
2. 在 `protocol` 新增请求转换与上游事件映射（纯数据，不发送网络）。
3. 在 `gateway::GatewayExecutor::stream_provider`（或 `provider.rs` 的
   发送分支）接线到 `upstream::UpstreamAccess` 发送。
4. 保持 stream 不整体 buffer，复用现有 `bytes` / `memchr` / SSE 解码模式。

## 新增一个编码客户端集成

1. 在 `clients` 实现配置渲染、原子写入与权限规则；CLI 保留用户提示，业务
   步骤走 `application` 用例。
2. 客户端密钥只通过既有存储路径流转，绝不写入日志或打印。
3. 服务启动与明确修复流程是密钥同步的合法触发点；不要把它放回全局命令分发。

## 修改网关热路径

- 请求转换、SSE 编解码是热路径：不要整体 buffer 响应、不要每请求重建
  client、不要引入热路径锁。
- 性能基线：`cargo run --release --example perf_probe`（SSE 解码、
  Anthropic 映射、Responses 转换吞吐）。持续退化超过 10% 需要消除。

## 维护架构检查

`scripts/check_arch.sh` 的规则应与本文档的模块职责保持一致；新增模块后
如涉及边界，先更新脚本再提交。

## 提交流程

`cargo fmt --all -- --check`、`scripts/check_arch.sh`、
`cargo test --locked --all-targets` 必须通过；涉及 prompt cache 时跑
`./scripts/e2e_prompt_cache.sh`，涉及多 provider 时跑
`./scripts/e2e_multi_provider.sh`。
