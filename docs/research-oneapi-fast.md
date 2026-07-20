# 百度 OneAPI 的 GPT / Anthropic 模型是否支持 `fast`

调研日期：2026-07-20（Asia/Taipei）

## 结论

**不能把“请求被接受”视为支持 Fast。需要区分三件事：**

1. `fast: true` 不是 OpenAI 官方 Responses API 的 Fast/Priority 开关；对百度 OneAPI 的一次实测返回了 `502 upstream_error`。
2. Codex 的 `/fast` 是产品功能。当前官方说明中，ChatGPT 登录态可用 `service_tier = "fast"` 配合 `[features].fast_mode = true`；API key 场景对应的是另行计费的 API Priority processing，而官方请求参数是 `service_tier: "priority"`，不是裸 `fast` 字段。[Codex Fast mode][codex-fast] [OpenAI Priority processing][openai-priority]
3. 百度 OneAPI 的 `/v1/responses` **能把部分请求实际处理为 Priority，但当前行为不是只由 GPT 模型名决定，也不稳定**：重复探针中，GPT-5.6 Luna、Terra、Sol 都分别出现过返回 `priority` 或 `default` 的观测；GPT-5.4、GPT-5.5 的全部本次观测均返回 `default`。因此只能认为 OneAPI 的部分 GPT-5.6 后端 route/channel 支持 Priority，不能宣称某个模型稳定支持。调用方必须读取响应中的 `service_tier` 回显。
4. Anthropic Messages 的 **Fast mode 和 Priority Tier 是两套独立机制**。Fast 使用 body `speed: "fast"` 加 `anthropic-beta: fast-mode-2026-02-01`；Priority 使用 body `service_tier: "auto"`（默认值），不是 `"priority"`。Fast 看 `usage.speed` 和 `anthropic-fast-*` headers，Priority 看 `usage.service_tier` 和 `anthropic-priority-*` headers。[Anthropic Fast mode][anthropic-fast] [Anthropic service tiers][anthropic-tiers]
5. 百度 OneAPI `/v1/messages` 对 Opus 4.8/4.7 的正确 Fast 请求均返回 HTTP 200，但重复样本没有 `usage.speed`，也没有任何 `anthropic-fast-*` header；非法 `speed` 同样可返回 200。Priority 的 `auto`、`standard_only` 甚至非法 tier 也都可返回 200，却没有 tier 回显或 priority headers。当前证据指向 route/channel 宽松忽略字段，**不能认为 Fast 或 Priority 已开启**。

对本仓库还有一个更直接的限制：`baidu-oneapi` preset 当前走 Anthropic `/v1/messages`，本地 Responses→Messages 转换既没有转发 `service_tier`，也没有生成 Anthropic `speed`。所以**现有 codex-mixin 链路无法把 Codex Fast/Priority 意图送到 OneAPI 上游**，即便某条 OneAPI route 能处理它。[preset 与路径][mixin-config] [请求结构][mixin-anthropic] [转换代码][mixin-convert] [调用链][mixin-server]

## 术语和官方语义

### Codex Fast mode

Codex 官方手册将 Fast mode 描述为提高受支持模型速度、同时提高 credits 消耗的产品功能。当前支持 GPT-5.6、GPT-5.5 和 GPT-5.4；CLI 使用 `/fast on|off|status`。持久化配置是：

```toml
service_tier = "fast"

[features]
fast_mode = true
```

同一说明明确区分了两种计费/接入方式：ChatGPT 登录态使用 credits；API key 使用 API token pricing，而 API Priority processing 有自己的费率。[Codex Fast mode][codex-fast]

### OpenAI API Priority processing

OpenAI 官方 API 文档规定，Responses 或 Completions 请求以如下字段请求 Priority：

```json
{"service_tier":"priority"}
```

响应对象的 `service_tier` 表示实际使用的 tier。文档也说明，在某些流量爬升条件下 Priority 请求可能被降级，届时响应会显示 `service_tier="default"`。因此判定是否生效应以**响应回显**为准，而不是 HTTP 200 或请求字段本身。[OpenAI Priority processing][openai-priority]

### Anthropic Messages Fast mode

Anthropic 的 Fast mode 与 OpenAI/Codex 的 `service_tier` 不是同一个 wire protocol。官方请求同时需要：

```http
anthropic-beta: fast-mode-2026-02-01
```

```json
{"speed":"fast"}
```

截至调研日，Fast mode 仍是 research preview，需要账户开通，支持 Claude Opus 4.8 和 Claude Opus 4.7；官方同时说明 Opus 4.7 Fast 已于 2026-06-25 deprecated，并将在 2026-07-24 移除。Opus 4.6 不再支持 Fast，但请求 `speed="fast"` 会以标准速度运行并在 `usage.speed` 回显 `standard`。[Anthropic Fast mode][anthropic-fast]

正确的生效判断是响应 `usage.speed` 为 `fast`；另有 `anthropic-fast-input-tokens-*` 与 `anthropic-fast-output-tokens-*` headers 显示独立的 Fast 限额。受支持模型在 Fast 容量不足时不会静默降为标准速度，而会返回 `429` 或 `529`；官方建议若要降级，由客户端捕获失败后去掉 `speed` 重试。不同 speed 不共享 prompt cache prefix。[Anthropic Fast mode][anthropic-fast]

Anthropic 官方 Python SDK 的 beta request type 也将 `speed` 限定为 `"standard" | "fast"`，usage 限定为相同回显，并列出 `fast-mode-2026-02-01` beta 常量，和文档一致。[SDK request type][anthropic-sdk-request] [SDK usage type][anthropic-sdk-usage] [SDK beta enum][anthropic-sdk-beta]

### Anthropic Messages Priority Tier

Anthropic Priority Tier 是容量承诺机制，不是 Fast mode。请求字段也是 `service_tier`，但合法请求值与 OpenAI 不同：

- `"auto"`（默认）：Priority 容量可用时使用，否则回落其他容量；
- `"standard_only"`：只使用 Standard。

实际 tier 位于响应 `usage.service_tier`，可能为 `priority`、`standard` 或 `batch`。当请求 `auto` 且组织有 Priority commitment 时，`anthropic-priority-input-tokens-*` 和 `anthropic-priority-output-tokens-*` headers 可用于判断资格与余额。当前官方页面说明新的 Priority capacity commitment 已停止销售，仅既有合同继续使用；模型支持也受具体 commitment 约束。[Anthropic service tiers][anthropic-tiers] [SDK request type][anthropic-sdk-request] [SDK usage type][anthropic-sdk-usage]

因此不能把 OpenAI 的 `service_tier="priority"` 原样转给 Anthropic；Anthropic 请求 Priority 的正确方式是 `service_tier="auto"`，而且它默认就是 `auto`。同样，Anthropic Fast 必须使用 `speed` 与 beta header，不能用 `service_tier="fast"` 替代。

## 百度 OneAPI 实测

### 方法

- 目标：百度 OneAPI 第一方 API `https://oneapi-comate.baidu-int.com`。
- 鉴权：当前用户有效 OneAPI token；报告未记录 token、完整响应 ID 或内部 request ID。
- 模型清单：2026-07-20 对 `GET /v1/models` 的结果包含 `gpt-5.4`、`gpt-5.5`、`gpt-5.6-luna`、`gpt-5.6-terra`、`gpt-5.6-sol`。
- 主要探针：`POST /v1/responses`，短输入、低输出上限，分别省略 tier、传 `service_tier="priority"`、`service_tier="fast"`、明显非法 tier，以及一次裸 `fast: true`。
- 补充探针：`POST /v1/messages`，比较省略 tier、`priority` 和非法 tier。
- 这是少量、时间点相关的黑盒探针，不等同于 OneAPI 的正式能力承诺；route/channel 池变化可能改变结果。

### `/v1/responses` 结果

| 请求 | 模型/范围 | HTTP | 响应 `service_tier` | 解释 |
| --- | --- | ---: | --- | --- |
| 省略 `service_tier` | GPT-5.5 | 200 | `default` | 基线为 Standard/default |
| `service_tier="priority"` | GPT-5.4 | 200 | 本次重复观测均为 `default` | 接受字段，但本次未实际使用 Priority |
| `service_tier="priority"` | GPT-5.5 | 200 | 本次重复观测均为 `default` | 接受字段，但本次未实际使用 Priority |
| `service_tier="priority"` | GPT-5.6 Luna | 200 | 不同探针出现 `priority` 和 `default` | 后端 route/channel 行为异构，不能按 model id 保证 |
| `service_tier="priority"` | GPT-5.6 Terra | 200 | 不同探针出现 `priority` 和 `default` | 后端 route/channel 行为异构，不能按 model id 保证 |
| `service_tier="priority"` | GPT-5.6 Sol | 200 | 不同探针出现 `priority` 和 `default` | 后端 route/channel 行为异构，不能按 model id 保证 |
| `service_tier="fast"` | GPT-5.5、Luna、Terra（样本） | 200 | `default` | 字面值 `fast` 被接受，但没有表现为 API Priority |
| 明显非法 `service_tier` | GPT-5.5、Sol（样本） | 502 | `upstream_error` | OneAPI 没有在网关层给出清晰的字段校验错误 |
| 裸 `fast: true` | GPT-5.6 Sol（单次） | 502 | 无 | 既非官方参数，本次也未成功 |

最关键的发现不是“哪些模型支持”，而是**相同的 GPT-5.6 模型名在不同请求中可能落到不同能力的后端**。这也解释了单次探针为何容易得出互相矛盾的模型级结论。

`service_tier="fast"` 返回 HTTP 200 但回显 `default`，尤其说明 OneAPI 对未知或非标准值的“宽松接受”不等于 honor 该能力。

### `/v1/messages` 结果

GPT-5.5 的省略 tier、`service_tier="priority"` 和明显非法 tier 探针均返回 HTTP 200；Anthropic Messages 响应没有 `service_tier` 字段可供确认。这个结果只能支持“该兼容入口宽松接受/忽略额外字段”，**不能证明它启用了 Priority**。

### `/v1/messages` 的 Anthropic Fast/ Priority 专项结果

专项探针按 Anthropic 官方 wire protocol 使用 `anthropic-version: 2023-06-01`。Fast 正确请求额外携带 `anthropic-beta: fast-mode-2026-02-01` 和 body `speed="fast"`；Priority 请求使用 body `service_tier="auto"`。

| 请求 | OneAPI 模型 | 重复/结果 | 生效证据 |
| --- | --- | --- | --- |
| 基线（无 speed） | `Opus 4.8` | HTTP 200 | usage 仅有 token/OneAPI credits 字段，无 `usage.speed` |
| `speed="fast"`，无 beta | `Opus 4.8` | HTTP 200 | 无 `usage.speed`，无 `anthropic-fast-*` headers |
| `speed="fast"` + 正确 beta | `Opus 4.8` | 多次均 HTTP 200 | 无 `usage.speed`，无 `anthropic-fast-*` headers |
| `speed="fast"` + 正确 beta | `Claude Opus 4.7` | 多次均 HTTP 200 | 无 `usage.speed`，无 `anthropic-fast-*` headers |
| 非法 `speed` | `Opus 4.8` | HTTP 200 | 无 `usage.speed`，未产生参数错误 |
| `speed="fast"` + 正确 beta | `Claude Opus 4.6`、`Claude Sonnet 4.6` | HTTP 200 | 无 `usage.speed`，无 `anthropic-fast-*` headers |
| `service_tier="auto"` | `Opus 4.8` | HTTP 200 | 无 `usage.service_tier`，无 `anthropic-priority-*` headers |
| `service_tier="standard_only"` | `Opus 4.8` | HTTP 200 | 无 tier 回显或 priority headers |
| 非法 `service_tier` | `Opus 4.8` | HTTP 200 | 无 tier 回显，未产生参数错误 |

另一次 `Fable 5` + `speed="fast"` 探针返回 HTTP 400 `invalid_request_error`；由于没有同 route 的配对基线，不能把该错误唯一归因于 `speed`，因此不作为支持或不支持 Fast 的主要证据。

结论是：OneAPI `/v1/messages` 当前样本表现为对 Opus/Claude route 的额外字段宽松忽略。即使 HTTP 200，也同时缺少 Anthropic 官方要求的 body 回显与限额 headers；现阶段没有证据表明这些 route 实际启用了 Anthropic Fast 或 Priority。由于 OneAPI 已在 OpenAI Responses 探针中表现出 route/channel 异构，后续若 OneAPI 上线该能力，也必须按固定 route/channel 重复验证，不能只检查模型名或 HTTP 状态。

## 对 codex-mixin 的影响

当前实现中：

1. `BaiduOneApi` 的默认协议是 `AnthropicMessages`，默认对话路径为 `/v1/messages`。[源码][mixin-config]
2. Responses→Anthropic 转换显式重建 `MessageRequest`，字段包括 model、max tokens、messages、tools、thinking、metadata 等，但没有读取或写入 `service_tier`。[源码][mixin-convert]
3. `/v1/responses` handler 对 OneAPI 自定义模型调用上述转换后再请求 Anthropic upstream。[源码][mixin-server]

因此，目前即使 Codex 请求携带 `service_tier="fast"` 或 `"priority"`，通过 `baidu-oneapi` preset 时也会在本地转换阶段丢失；转换后的 `MessageRequest` 也没有 `speed` 字段。[请求结构][mixin-anthropic]

如果采用“上游 API 能开就开”的策略，建议按协议而不是按统一字段实现：

1. 把 Codex 的 Fast 选择视为本地统一意图，不原样透传 wire value。
2. GPT/OpenAI Responses 路由发送 `service_tier="priority"`，保留响应的实际 `service_tier`；返回 `default` 时继续使用结果，但记为未生效/被降级。
3. Anthropic Messages 只对仍受官方支持且 OneAPI 实际提供的 Opus Fast 模型发送 `speed="fast"`，并合并 `fast-mode-2026-02-01` beta header。只有 `usage.speed="fast"` 才算开启成功。
4. Anthropic Fast 遇到无权限/不支持错误，或 `429`、`529`，可按产品策略去掉 `speed` 重试一次 Standard；回退会造成 prompt-cache miss。不要把 `service_tier="priority"` 传给 Anthropic。
5. Anthropic Priority 的 `service_tier="auto"` 本来就是默认值，当前 OneAPI 又不回显实际 tier，因此无需为了“Fast”额外注入它。

由于当前 OneAPI Claude route 会静默忽略正确 Fast 参数，建议先实现协议映射、回显和 fallback，但把“已生效”状态严格绑定到上游回显，而不是请求是否成功。

## 最终判断

| 问题 | 判断 |
| --- | --- |
| OneAPI 支持裸 `fast: true` 吗？ | **没有证据支持；官方 API 也没有这个开关，本次单次探针为 502。** |
| OneAPI 支持 `service_tier="fast"` 吗？ | **字段可被接受，但样本均实际返回 `default`；不能视为支持。** |
| OneAPI GPT 支持 API Priority 吗？ | **部分 GPT-5.6 route/channel 实测能返回 `priority`，但同模型也会返回 `default`，目前不能保证。GPT-5.4/5.5 本次仅观测到 `default`。** |
| OneAPI Claude/Opus 支持 Anthropic Fast 吗？ | **当前没有生效证据；正确的 `speed` + beta 请求仍缺少 `usage.speed`/Fast headers，非法值也被接受，表现为字段被忽略。** |
| Anthropic Priority 能代替 Fast 吗？ | **不能；它是另一套容量服务，合法请求值是 `auto`/`standard_only`，而且 `auto` 已是默认。** |
| 当前 codex-mixin 能用到它吗？ | **不能；Baidu preset 走 `/v1/messages`，转换层既丢弃 `service_tier`，也没有 Anthropic `speed` 映射。** |

[codex-fast]: https://learn.chatgpt.com/docs/agent-configuration/speed#fast-mode
[openai-priority]: https://developers.openai.com/api/docs/guides/priority-processing
[anthropic-fast]: https://platform.claude.com/docs/en/build-with-claude/fast-mode
[anthropic-tiers]: https://platform.claude.com/docs/en/api/service-tiers
[anthropic-sdk-request]: https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/beta/message_create_params.py
[anthropic-sdk-usage]: https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/beta/beta_usage.py
[anthropic-sdk-beta]: https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/anthropic_beta_param.py
[mixin-config]: https://github.com/Edward-lyz/codex-mixin/blob/95ee4ec1c3a28881f1d500b7a84673569eb4d952/src/config.rs#L39-L89
[mixin-anthropic]: https://github.com/Edward-lyz/codex-mixin/blob/95ee4ec1c3a28881f1d500b7a84673569eb4d952/src/anthropic.rs#L4-L25
[mixin-convert]: https://github.com/Edward-lyz/codex-mixin/blob/95ee4ec1c3a28881f1d500b7a84673569eb4d952/src/convert.rs#L118-L216
[mixin-server]: https://github.com/Edward-lyz/codex-mixin/blob/95ee4ec1c3a28881f1d500b7a84673569eb4d952/src/server.rs#L645-L681
