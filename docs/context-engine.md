# 长上下文引擎（Context Engine）

默认关闭。开启后，网关为每个会话维护「原始消息归档 + 压缩视图」两层结构：
客户端仍然只看到自己的对话，压缩由模型自主发起的四个内部工具驱动，Rust
侧负责校验范围、状态与持久化。

## 开启与配置

`bash
codex-mixin context config set --enabled true
codex-mixin context config get --json
`

可调项（默认值见 `src/config/context.rs`）：

| 配置 | 含义 | 默认 |
| --- | --- | --- |
| `enabled` | 总开关 | `false` |
| `context_window_tokens` | 视图窗口，未设置时取网关的上下文窗口 | 网关默认 |
| `growth_threshold_tokens` | 视图增长到该 token 数后提示模型压缩 | 50000 |
| `pressure_ratio` / `urgent_ratio` | 提示与强提示的窗口占比 | 0.75 / 0.95 |
| `protected_messages` | 最近保留的消息条数 | 5 |
| `protected_tokens` | 最近保留的 token 数 | 5000 |
| `max_internal_rounds` | 单次用户请求内的内部续轮上限 | 4 |
| `max_tier` | 允许产生的最高压缩层级（1-3），用于消融实验 | 3 |
| `guidance_only` | 只注入压缩指引、不注入压缩工具（仅消融实验） | `false` |

CLI 只改配置，不启动网关：`codex-mixin context config set --growth-threshold-tokens 40000`。
窗口参数接受 `none`（清除）与正整数（0 报错）。消融实验用
`--enabled false`、`--guidance-only true`、`--max-tier 1` 和默认配置四组区分
原有流程、仅指引、仅 T1 与完整三级压缩；`max_tier` 超限时压缩失败并返回
`compression tier N exceeds the configured max_tier M`。

## 模型侧接口

网关在发给上游的请求里追加 `<context_management>` 指引与四个工具，
调用结果只回传给模型，客户端流里不会出现：

- `mixin_context_compress`：把若干连续 segment 压缩成一条摘要（可选 `ranges` 或 `block_ids`）。
- `mixin_context_decompress`：查看或恢复被压缩的 block。
- `mixin_context_search`：在原始消息与 block 中检索。
- `mixin_context_status`：查看 revision、窗口占用、可压缩范围与保护原因。

压缩分三级：原始消息 → T1 摘要 → T2 摘要合并 → T3 再合并。范围必须连续、
不重叠、至少两段，且不能触碰受保护内容。

## 保护与失败语义

受保护内容：system/developer、首条与最新用户消息、最近 `protected_messages`
条与 `protected_tokens` 个 token、未完成的工具调用，以及成对的调用与结果。
触碰保护或范围非法时返回结构化错误，不截断上下文。摘要没有带来 token 收缩时
同样拒绝，避免把视图压大。

每个会话带 revision：模型基于旧视图提交的压缩会得到
`context revision changed from N to M`，调用方需要重新读取状态。
内部续轮超限时返回 `response.failed`，错误为
`context internal rounds exhausted before the model answered`。

## 存储与会话

数据存放在独立 SQLite（WAL），路径由 `context config get --json` 的
`database_path` 给出，也可用 `CODEX_MIXIN_CONTEXT_DB` 覆盖：

`bash
codex-mixin context status --session <id> --json
codex-mixin context sessions list --json
codex-mixin context sessions show <id> --json
codex-mixin context export --session <id> --out context-export.json
`

会话键由接入客户端与客户端提供的 thread/session id 组合而成，子 agent 使用
独立键，互不影响。网关不会在模型请求期间锁库：读视图、提交压缩都是短事务。

## 链式请求（previous_response_id）

客户端只发增量、用 `previous_response_id` 续接时，网关不会把该 id 透传给上游：
它用归档的原始消息加上已提交的上一轮可见输出重建完整入参，再按当前 revision 渲染
压缩视图。因此压缩/恢复之后的续轮始终从完整视图重新开始，不会命中过期的
response id。

映射按会话隔离并只在轮次成功后提交：跨会话的 id 返回
`response id ... belongs to context session ...`，未知 id 且没有任何历史时返回
`unknown response id ...`（HTTP 400）；未知 id 但客户端已带完整历史时按客户端历史
继续，避免丢掉客户端自己的对话。

## 压缩接管（compaction）

开启后，`/v1/responses/compact` 与带 `compaction_trigger` 的请求不再让模型写摘要：
网关先把入参同步进归档，再返回本地 checkpoint（令牌前缀
`codex-mixin:context:v2:`），内容只有会话、revision、模型与签发时间，并带 AEAD
认证。整个请求不会调用任何上游模型。

checkpoint 绑定会话与 revision：

- 换一个会话重放返回 `compaction checkpoint belongs to context session ...`（HTTP 400）；
- 令牌 revision 比归档更新（例如库被回滚）返回 `compaction checkpoint revision ...
  is newer than the archived session revision ...`，不会静默丢消息；
- 篡改或跨密钥的令牌无法解封，前缀不对返回 `unsupported context checkpoint`。

客户端带着 checkpoint 续轮时，网关在调用上游前把它展开成归档原文（客户端可能
已经丢弃的历史）再加上 checkpoint 之后的新消息；指纹仍与归档一致，所以压缩块继续
生效。本地令牌绝不会出现在发往任何上游的报文里，官方服务也不需要解释它。v1
`codex-mixin:compaction:v1:` 令牌保持可读。

关闭开关后不再签发新 checkpoint，原流程（模型摘要或官方 compact）恢复；但归档已
存在的会话仍能展开并读取 checkpoint：此时不注入内部工具与指引、只跑一轮，历史照常
保留。

## 控制台与统计

Web 控制面板的「上下文」页把开关与统计放在一起：顶部开关直接调用
`context config set`，其余卡片展示会话数、压缩次数、节省 token 与缓存命中，
下方表格按会话列出消息数、压缩次数、节省量、当前视图 token 与缓存命中率。
开关与其他设置只在网关重启后生效，页面提供「重启网关」。

数据来自 `context stats --json`（同一份数据也可在终端查看）：

```bash
codex-mixin context stats --json
```

```json
{
  "enabled": true,
  "database_path": ".../context.db",
  "totals": {
    "sessions": 1,
    "messages": 132,
    "compressions": 4,
    "saved_tokens": 81234,
    "view_tokens": 24000,
    "cache": {
      "turns": 18,
      "input_tokens": 420000,
      "cached_tokens": 310000,
      "output_tokens": 21000,
      "hit_percent": 73.8
    }
  },
  "sessions": [
    { "session": "codex\u001fthread-1", "display": "codex / thread-1", "revision": 9,
      "updated_at": 1790000000, "messages": 132, "compressions": 4,
      "saved_tokens": 81234, "view_tokens": 24000, "tiers": {}, "cache": {} }
  ]
}
```

- `compressions` 含已被更高层合并的压缩；`saved_tokens` 只统计当前视图里仍然
  生效的压缩块（原始片段 token 减去摘要 token）。
- `cache` 是网关按上下文会话记录的真实上游用量：每个上游轮次（含内部续轮）
  记一条 `input_tokens`、`input_tokens_details.cached_tokens` 与
  `output_tokens`，命中率是缓存读取除以计费输入。关闭开关后只读路径不记账。
- 归档库尚未创建时 `stats` 返回空会话与全零 totals，且不会新建数据库。

## 范围

本版本提供内核、存储、网关回路、链式请求、压缩接管、CLI 统计与 Web 控制面板的
「上下文」页；macOS 与 Windows 壳的设置区、以及评估基线尚未包含。