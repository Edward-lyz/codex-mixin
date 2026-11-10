# 模型路由

模型路由回答一个问题：客户端请求模型 A 时，网关实际把请求交给谁。每条路由由
**入口模型**（客户端请求的模型）标识，共三种：

| 类型 | 入口模型 | 行为 | 存储 |
|---|---|---|---|
| 融合 `fusion` | 虚拟模型 `auto/<id>` | 会话中每个新用户轮次并行询问 1–8 个面板模型（附带有界的历史对话），裁判模型整合，最终模型作答；同一轮内的工具续调直接走最终模型 | `fusion_profiles`，`mode: orchestration` |
| 定时 `schedule` | 虚拟模型 `auto/<id>` | 按本地时间选择模型（1–24 个不重叠时段，跨零点可用），时段外走默认模型 `final_model` | `fusion_profiles`，`mode: time_rotation` |
| 故障转移 `failover` | 已有 provider 模型或 `official:<slug>`；设置 `id` 时另有虚拟模型 `auto/<id>`（源必须是 provider 模型） | 源模型失败时按顺序改用 1–4 个备用模型 | `failover_routes` |

模型引用统一为 provider 目录 slug 或 `official:<slug>`；路由不能引用虚拟模型，
因此不会形成环。旧入口 `mixin/fusion/<id>` 仍可解析。

## 故障转移规则

故障转移是显式回退，触发条件、上限和可见状态都是固定的：

- 触发：上游返回 429、5xx，或连接失败、超时，且尚未向客户端输出任何内容。
  400 等客户端错误不会触发。
- 上限：按顺序尝试备用列表，每个备用模型最多一次，备用模型自身的故障转移不级联。
- 可见：每次切换写 `warn` 日志 `model failover`；成功后流的第一个事件是一条
  reasoning summary 状态（`item_id: failover_status`），写明哪个模型失败、改用谁。
  客户端看到的 `model` 字段保持为请求的模型。
- 覆盖范围：所有经 `GatewayExecutor::send_round` 的请求，包括 Responses HTTP/WS、
  Chat Completions、Gemini、Anthropic 转 Responses 以及上下文引擎的内部回合。
  官方模型配置故障转移后不再走透传，改走网关计划路径以便观察上游状态。
  Anthropic 原生透传（不经过请求计划）不在覆盖范围内。

## CLI

```sh
codex-mixin route list --json
codex-mixin route models --json
codex-mixin route get --model auto/night --json
codex-mixin route set --route-json '{"kind":"failover","source":"official:gpt-5.5","fallbacks":["deepseek-v4"]}'
codex-mixin route set --replace-model auto/old --route-json '{"kind":"schedule","id":"night",...}'
codex-mixin route delete --model official:gpt-5.5
```

`--route-json` 是带 `kind` 标签的对象：`fusion` / `schedule` 的其余字段与
`FusionProfile` 相同（`mode` 由 `kind` 决定），`failover` 为 `{source, fallbacks}`。
`list` 与 `get` 的每一项额外带 `model`（入口模型）。`--replace-model` 用于编辑时
改名或改类型，在一次提交中替换旧路由。保存只写配置；生效需要
`service restart --managed` 或 `config apply`。

旧的 `fusion get|set|delete|models` 子命令保留兼容。`interface --json` 的
`capabilities.model_routes` 表示可用 `route` 子命令。

## 控制台

macOS 与 Windows 控制台的「模型路由」页统一管理三种路由：左侧列出现有路由
（入口模型与目标摘要），点「新建」选择融合、定时或故障转移，右侧表单只编辑
该类型需要的字段。保存调用 `route set`，删除调用 `route delete`，随后自动
`config apply` 重启网关并刷新 Codex 模型目录。菜单栏与托盘中原来的「Fusion 设置」
入口改为打开这一页；Windows 的 `--page=fusion` 仍兼容并映射到模型路由页。
