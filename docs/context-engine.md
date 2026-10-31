# 长上下文引擎（Context Engine）

![内置压缩与归档式分层上下文管理](assets/context-engine-comparison.svg)

图示仅对比机制，不代表实测性能。用户侧的作用说明、默认配置、命令与排障见
[上下文引擎使用指南 / Wiki 源稿](wiki/Context-Engine.md)。

**适用路径**：引擎运行于共享 Responses 请求回路，需要会话标识与支持的 input
或链式续接。原生 Anthropic Messages 请求在启用引擎、有会话标识且内容可映射时
也接入该回路，保留 thinking 签名、beta、原生选项与 Messages 响应。托管工具和
无法保真映射的原生内容块继续直通，不启用压缩。因此开启全局开关不代表所有请求都参与；
下文的 compact 接管特指 Responses compact / compaction_trigger。

默认关闭。开启后，网关为每个会话维护「原始消息归档 + 压缩视图」两层结构：
客户端仍然只看到自己的对话，压缩由模型自主发起的四个内部工具驱动，Rust
侧负责校验范围、状态与持久化。

## 开启与配置

```bash
codex-mixin context config set --enabled true
codex-mixin context config set --compression-mode aggressive
codex-mixin context config get --json
```

macOS、Windows 和 Web 设置页统一提供「激进 / 平和」两档，不展示内部阈值。
平和是默认档；激进将增长门槛降到 25000 tokens、压力/紧急占比降到 0.6/0.85，
平和使用下表默认值。CLI 接受 `--compression-mode aggressive|gentle`，与三个阈值参数互斥；
两档只改变这三个阈值，保留保护区、内部轮次和显式窗口覆盖。`config get/set --json`
在顶层返回 `compression_mode`，GUI 只传档位名；已有自定义阈值不因无关保存而重置，
低于默认门槛的自定义配置显示为激进，其余显示为平和。
跟随模型窗口默认开启（`context_window_tokens: null`），关闭后才显示手动窗口输入。

可调项（默认值见 `src/config/context.rs`）：

| 配置 | 含义 | 默认 |
| --- | --- | --- |
| `enabled` | 总开关 | `false` |
| `context_window_tokens` | 显式视图窗口；未设置时解析具体模型元数据，缺项时用网关默认值 | 自动 |
| `growth_threshold_tokens` | T1 待压缩量门槛；同时用于推导增长间隔 | 50000 |
| `pressure_ratio` / `urgent_ratio` | 提示与强提示的窗口占比 | 0.75 / 0.95 |
| `protected_messages` | 最近保留的消息条数 | 3 |
| `protected_tokens` | 最近保留的 token 数 | 5000 |
| `max_internal_rounds` | 单次用户请求内的内部工具轮次上限 | 10 |
| `max_tier` | 允许产生的最高压缩层级（1-3），用于消融实验 | 3 |
| `guidance_only` | 只注入压缩指引、不注入压缩工具（仅消融实验） | `false` |

CLI 保存实际改动后自动重启正在运行的网关一次，例如
`codex-mixin context config set --growth-threshold-tokens 40000`。
未改动的设置不重启；已停止的网关保持停止。保存成功但重启失败时，CLI 返回非零退出码，
`--json-errors` 的错误标记 `committed: true` 与 `stage: "context gateway restart"`，设置不会回滚。
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

每轮请求在 input 末尾追加一条 `<context_state>` developer 消息，说明当前视图
估算 token、窗口占用百分比、增长/压力/紧急阈值的触发原因，以及最早可压缩范围
（如 `m000002..m000013 (t1)`）。它放在 input 末尾而不是 instructions 里：instructions
每轮变化会让上游前缀缓存整体失效，追加消息只延长前缀。模型据此自行决定何时调用
压缩工具，网关不替它选择摘要或强制压缩。触发决策与分层摘要规则移植自
[Billion Context](https://github.com/ranxianglei/billion-context/tree/94f75ccea1b2e509a29ca46e44768dcf9364940f)，
对应 `kernel/src/compress.ts` 的 `decideNudge` / `nudgeNode` 和
`kernel/src/compression-rules.ts`，遵守其 MIT 许可证（见
[`src/context/prompts/LICENSE`](../src/context/prompts/LICENSE)）。

- 增长从持久化的低水位基线或上次提示计算，首次低压力请求只建立基线。增长间隔为
  `max(20000, ceil(growth_threshold_tokens × 0.45))`，默认 22500 tokens。
  忽略提示且没有新增长时，不因总长度超过 50000 而反复催促。
- 初次出现的积压在窗口占用至少 45%、某层待压缩量至少达到增长门槛时可以立即触发。
  压缩成功在同一 SQLite 事务内清除基线，剩余积压可以重新评估。
- T1 只计入实际文本长度至少 5000 个 UTF-16 单元的可执行连续范围；T2/T3 计入
  活跃 T1/T2 摘要的 token 量。普通增长路径优先 T1，其次 T2、T3；后两层的量级门槛
  默认是 75000 tokens，并须大于低层待压缩量，或达到 1000/2000 个源块的数量门槛。
- 达到 75% 压力或 95% 紧急门槛时，选择待压缩量最多的层，但仍要求至少
  `max(5000, round(window × 0.01))` tokens 的潜在收益。
- 没有可执行范围或收益不足时继续用户任务。仅指引消融模式不提示工具调用或更新基线。

静态指引包含原版的压缩原则和 T1 保真规则；触发时再附加相应层级规则。未完成目标必须
以带原始消息引用的 `Open objectives:` 继续保留，T2/T3 不得因摘要合并丢失它们。
移植范围是提示决策、摘要规则与内部循环退出；Rust 仍使用现有连续 segment、保护、
非收缩拒绝、归档与恢复接口，并非引入整个 TypeScript kernel。摘要合并仍须存在
至少两个连续且未受保护的 segment；不可执行的层不会被提示。已保存的用户配置保持原值。

HTTP 与 WebSocket（含官方模型）复用同一上下文回路。可见文本事件增量转发，内部工具
事件被隐藏；同轮出现客户端工具时，先完成内部提交，再交回客户端调用。暂存的客户端
工具事件上限为 8 MiB，超限返回 `response.failed`，不会无界缓冲整轮 SSE。内部续轮对
客户端保持同一 response id、单一终止事件和递增事件编号；HTTP 断连会释放请求租约与上游流。

向 Anthropic Messages 上游回放时，隐藏内部工具轮留下的相邻 assistant 消息会先合并，
再将其中的 thinking / redacted_thinking 稳定归一化为开头的连续块。
例如 `[thinking₁, 文字₁, thinking₂, 文字₂, tool_use]` 变为
`[thinking₁, thinking₂, 文字₁, 文字₂, tool_use]`，避免 OneAPI 返回
`multiple signed blocks lack recorded continuation provenance`。
全部 thinking 内容、签名和原生块字段保持完整，文字与工具块之间的顺序不变；
原本已归一化的前缀不变。Responses 转换及启用上下文引擎的原生 Messages 重建均执行
这一步。旧版本保存的会话、链式续接和 checkpoint 展开也自动处理，包括关闭引擎后的
checkpoint 只读回放；不需要清库、重开会话或迁移原始消息。

压缩分三级：原始消息 → T1 摘要 → T2 摘要合并 → T3 再合并。范围必须连续、
不重叠、至少两段，且不能触碰受保护内容。

## 保护与失败语义

受保护内容：system/developer、首条与最新用户消息、最近 `protected_messages`
条与 `protected_tokens` 个 token、未完成的工具调用，以及成对的调用与结果。
触碰保护或范围非法时返回结构化错误，不截断上下文。摘要没有带来 token 收缩时
同样拒绝，避免把视图压大。

每个会话带 revision：模型基于旧视图提交的压缩会得到
`context revision changed from N to M`，调用方需要重新读取状态。
到达内部工具轮次上限时，按原版 Responses adapter 发送 `response.completed`，
保留已产生的可见内容，不追加一次额外模型请求。若一轮全部是失败的 compress/decompress，
且重试了之前已经失败的相同参数，也会提前正常结束。没有文本产出的工具轮可能以空输出
结束；这表示循环已结束，不表示用户任务已完成。上游传输错误仍保持原有失败语义。

历史重放时，`reasoning.content` 缺省与 `null` 视为同一内容；非空内容仍按实际变化
处理。打开旧归档库时一次性规范化已有 reasoning 指纹，不改原始消息。
若编辑或截断发生在压缩块中间，该块的摘要失效，但变化点之前的原文会从归档恢复，
位于变化点之前的完整摘要块继续保留。同步历史时还会补齐旧版本留下的视图缺口，
提升 revision 并记录恢复消息数；这类会话首次恢复时，模型可见上下文会增大。

## 存储与会话

数据存放在独立 SQLite（WAL），路径由 `context config get --json` 的
`database_path` 给出，也可用 `CODEX_MIXIN_CONTEXT_DB` 覆盖：

```bash
codex-mixin context status --session <id> --json
codex-mixin context sessions list --json
codex-mixin context sessions show <id> --json
codex-mixin context export --session <id> --out context-export.json
```

会话键由接入客户端与客户端提供的 thread/session id 组合而成，子 agent 使用
独立键，互不影响。网关不会在模型请求期间锁库：读视图、提交压缩都是短事务。
请求与 compact 共用会话租约；查询统计或另开数据库连接不会清理有效租约。
正常结束、失败或取消会释放租约，进程崩溃后的遗留租约最迟在 30 分钟 TTL 后失效。

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
认证。整个请求不会调用任何上游模型。重复 compact 时先校验、展开已有 checkpoint，
再同步完整历史；跨会话或损坏的 checkpoint 在改写历史前返回 HTTP 400。

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
Windows、macOS 与 Web 使用同一保存流程：CLI 保存后自动重启运行中的网关使设置生效，
无需再单独点击重启；无改动时不重启，网关原本停止时仅保存。

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

## 消融评估

`scripts/evaluate_context.mjs` 用真实 Provider 跑四组对照，每组独立配置、独立归档库、
独立网关进程，任务集相同：

| 组 | 配置 | 作用 |
| --- | --- | --- |
| baseline | `context enabled=false` | 原有流程，不注入任何指导 |
| guidance | `enabled=true, guidance_only=true` | 只有压缩指导，不注入内部工具（也不建议压缩） |
| t1 | `enabled=true, guidance_only=false, max_tier=1` | 只允许一级压缩 |
| full | `enabled=true, guidance_only=false, max_tier=3` | 完整 T1→T2→T3 |

公共配置固定为 `--growth-threshold-tokens=8000 --protected-messages=2
--protected-tokens=500 --max-internal-rounds=4 --context-window-tokens=<EVAL_WINDOW>`。

```bash
EVAL_TASKS=3 EVAL_ROUNDS=2 EVAL_CHUNKS=48 EVAL_WINDOW=16000 \
  node scripts/evaluate_context.mjs
```

脚本自己挑空闲端口，从 `~/.codex-mixin/config.json` 复制一份配置到工作目录
（可用 `EVAL_SOURCE_CONFIG`/`EVAL_SOURCE_KEY` 换源），用 `context config set` 写组配置，
跑完 SIGTERM 停进程，最后打印 Markdown 表并把原始数据写到
`<EVAL_WORKDIR>/results.json`（默认 `$TMPDIR/codex-mixin-context-eval`）。
任务分三类轮换：长构建日志里埋精确错误码、多文件改动里埋文件路径、目标变更里埋最新目标；
每类都要模型只回埋进去的那个事实，用于判断关键事实是否被压掉。

以下是本次算法对齐前的历史运行，未用新实现重跑，不作为本次修复的质量证明。
本机一次真实运行（模型 `DeepSeek-V4.1-Flash-baidu-oneapi`，窗口 16000，三类任务各 48 段
2000 字符日志 + 2 轮填充）：

| 组 | 关键事实命中 | 输入 token | 缓存 token | 输出 token | 首 token (ms) | 压缩次数 | 节省 token |
| --- | --- | --- | --- | --- | --- | --- | --- |
| baseline | 3/3 | 219402 | 216960 | 754 | 1505 | 0 | 0 |
| guidance | 3/3 | 220066 | 144384 | 545 | 1445 | 0 | 0 |
| t1 | 3/3 | 30923 | 19968 | 717 | 1193 | 3 | 41502 |
| full | 3/3 | 29829 | 17408 | 565 | 930 | 4 | 55915 |

读法：四组关键事实都是 3/3，没有因为压缩丢事实；t1/full 的总输入约为 baseline 的
1/7，full 的总输入、缓存读取和节省 token 都优于 t1（29829 < 30923，17408 < 19968）；
但按计费输入（输入减缓存）看 t1 是 10955、full 是 12421，这一项 t1 更低。收益只来自
被折叠掉的历史，压缩块本身和摘要写入仍然要付费，所以摘要越长、折叠越少时 t1 更划算。

限制要说清楚：填充内容是合成日志，重复度高，压缩比因此偏乐观；只跑了一个模型、一台机器、
每组一次；内存与真实用户任务未测；首 token 是三类任务里的最小值。这些数字不构成对
其他模型或真实仓库的承诺。

## 范围

本版本提供内核、存储、网关回路、链式请求、压缩接管、CLI 统计、Web 控制面板的
「上下文」页，以及 macOS 控制台与 Windows 独立窗口的「上下文」设置区；四组消融
实验脚本见 `scripts/evaluate_context.mjs`，跑法与结果见上文「消融评估」。

## 工具调用边界校验

压缩范围必须同时包含 `function_call` / `custom_tool_call` 和对应的
`function_call_output` / `custom_tool_call_output`。即使两端都不属于最近消息
保护区，也不能只压缩其中一端。校验失败返回
`compression would split tool call ... from its output`，不改变归档或 revision。
模型需要调整范围，完整覆盖调用与返回；整对压缩后仍可通过恢复整个摘要块还原。
这防止了可见历史中出现孤立工具返回，以及上游的 `No tool call found ...` 错误。
