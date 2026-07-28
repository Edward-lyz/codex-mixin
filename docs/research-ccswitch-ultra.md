# CC Switch 对第三方 Codex Ultra 模式的支持调查

调查日期：2026-07-27

## 结论

CC Switch 的**已发布版本 v3.18.0 和当前主分支尚不能通过正常的 Provider Model Mapping 给第三方模型暴露 Ultra**。

但这不是协议层做不到。官方仓库已有开放 PR
[#5265](https://github.com/farion1231/cc-switch/pull/5265)，它为第三方
`gpt-5.6-sol`、`gpt-5.6` 和 `gpt-5.6-terra` 生成
`low / medium / high / xhigh / max / ultra` 六档 catalog 元数据。PR 作者记录了
Codex Desktop 实机验证；本次调查也在 PR head
`094b3324a790d3eb814b377e1a51a46018ee73da` 上运行了对应 Rust 单测，测试通过。

不过，这个 PR 目前只解决了 **Ultra 出现在 picker 中**，没有复制官方 Sol/Terra
的 `multi_agent_version: "v2"`。OpenAI Codex 源码表明，只有
`MultiAgentVersion::V2` 才会把 Ultra 解释为 `Proactive` multi-agent mode。
因此不能把 PR 的“六档可见”直接等同于完整 Ultra 行为；在没有全局 feature
override 的通常 catalog 驱动路径下，它还缺一块关键元数据。

请求侧则不需要 CC Switch 重写：OpenAI Codex 会把内部 Ultra 转为上游
`reasoning.effort = "max"`，同时在客户端把 multi-agent mode 设为
`Proactive`。第三方上游不应收到字面值 `ultra`。

因此，对 `codex-mixin`/Baidu OneAPI 的启示是：自定义 GPT catalog 需要同时
声明 Ultra 和 `multi_agent_version: "v2"`，并保持 GPT 走原生 Responses。
无需让 OneAPI 接受字面值 `ultra`。

## 调查对象

- 官方仓库：[farion1231/cc-switch](https://github.com/farion1231/cc-switch)
- 最新正式版：[v3.18.0](https://github.com/farion1231/cc-switch/releases/tag/v3.18.0)
- 调查时主分支 commit：
  [`708b38791cec070e634b2364958d6afba9000770`](https://github.com/farion1231/cc-switch/tree/708b38791cec070e634b2364958d6afba9000770)
- Ultra 支持 PR head：
  [`094b3324a790d3eb814b377e1a51a46018ee73da`](https://github.com/farion1231/cc-switch/tree/094b3324a790d3eb814b377e1a51a46018ee73da)
- 本地源码：`/tmp/cc-switch-research`

## 已发布版本为何没有 Ultra

CC Switch 当前把 ProxyChat 类型第三方模型建立在硬编码的 `gpt-5.5`
模板上：

- 模板 slug 固定为 `gpt-5.5`：
  [`codex_config.rs#L115`](https://github.com/farion1231/cc-switch/blob/708b38791cec070e634b2364958d6afba9000770/src-tauri/src/codex_config.rs#L115)
- 模板优先从 Codex cache/CLI 读取，最后回退到内置静态模板：
  [`codex_config.rs#L928-L945`](https://github.com/farion1231/cc-switch/blob/708b38791cec070e634b2364958d6afba9000770/src-tauri/src/codex_config.rs#L928-L945)
- 生成第三方条目时复制整个模板，只覆盖 slug、名称、context 等字段，
  没有覆盖 reasoning levels：
  [`codex_config.rs#L464-L496`](https://github.com/farion1231/cc-switch/blob/708b38791cec070e634b2364958d6afba9000770/src-tauri/src/codex_config.rs#L464-L496)
- 内置 `gpt-5.5` 模板只到 `xhigh`：
  [`gpt5_5_template.json#L5-L23`](https://github.com/farion1231/cc-switch/blob/708b38791cec070e634b2364958d6afba9000770/src-tauri/src/resources/gpt5_5_template.json#L5-L23)

原生 Responses 和 Anthropic 类型更保守：它们使用独立模板，只暴露
`none / high`：

- profile 选择：
  [`codex_config.rs#L986-L1004`](https://github.com/farion1231/cc-switch/blob/708b38791cec070e634b2364958d6afba9000770/src-tauri/src/codex_config.rs#L986-L1004)
- 原生模板：
  [`codex_native_responses_template.json#L6-L16`](https://github.com/farion1231/cc-switch/blob/708b38791cec070e634b2364958d6afba9000770/src-tauri/src/resources/codex_native_responses_template.json#L6-L16)

所以在 v3.18.0/main 中，换用第三方 Provider 后看不到 Ultra 是 catalog
生成策略导致的，不是 CC Switch 已探测到上游不支持。

## 开放 PR 如何实现 Ultra

PR #5265 不再让已知 GPT-5.6 模型盲目继承模板 reasoning levels，而是按模型
显式写入：

- `gpt-5.6` / `gpt-5.6-sol`：
  `low, medium, high, xhigh, max, ultra`
- `gpt-5.6-terra`：
  `low, medium, high, xhigh, max, ultra`
- `gpt-5.6-luna`：
  `low, medium, high, xhigh, max`
- `gpt-5.5`：
  `low, medium, high, xhigh`

实现见
[`codex_config.rs#L1416-L1467`](https://github.com/farion1231/cc-switch/blob/094b3324a790d3eb814b377e1a51a46018ee73da/src-tauri/src/codex_config.rs#L1416-L1467)，
生成每个 catalog entry 时应用该元数据：
[`codex_config.rs#L1490-L1512`](https://github.com/farion1231/cc-switch/blob/094b3324a790d3eb814b377e1a51a46018ee73da/src-tauri/src/codex_config.rs#L1490-L1512)。

测试明确断言 Sol、其别名和 Terra 都有六档：
[`codex_config.rs#L4149-L4205`](https://github.com/farion1231/cc-switch/blob/094b3324a790d3eb814b377e1a51a46018ee73da/src-tauri/src/codex_config.rs#L4149-L4205)。

PR 描述还记录：在 CC Switch 3.16.5 和 Codex Desktop
`OpenAI.Codex_26.707.3748.0` 上，切换 Provider 并重启 Codex 后，
第三方 `gpt-5.6-sol` 显示六档 reasoning levels。它没有记录是否观察到
Ultra 主动派生 subagent。该 PR 截至调查时仍为 `OPEN`，不是 v3.18.0
的已发布能力。

本次独立验证命令：

```bash
cargo test --manifest-path src-tauri/Cargo.toml \
  codex_model_catalog_uses_model_specific_reasoning_levels --lib
```

在 PR head `094b3324...` 上退出码为 0。

## 完整 Ultra 还需要 multi_agent_version

官方 OpenAI Codex commit
[`d6ea5991e7f91eec5bad1fb4cb36f6be638878bc`](https://github.com/openai/codex/tree/d6ea5991e7f91eec5bad1fb4cb36f6be638878bc)
提供了完整语义的直接证据：

- 官方 `gpt-5.6-sol` catalog 同时包含 `multi_agent_version: "v2"` 和
  “Maximum reasoning with automatic task delegation”的 Ultra：
  [`models.json#L4-L22`](https://github.com/openai/codex/blob/d6ea5991e7f91eec5bad1fb4cb36f6be638878bc/codex-rs/models-manager/models.json#L4-L22)、
  [`models.json#L48-L58`](https://github.com/openai/codex/blob/d6ea5991e7f91eec5bad1fb4cb36f6be638878bc/codex-rs/models-manager/models.json#L48-L58)。
- Codex 只有在 runtime 为 V2 时才产生 multi-agent mode；effort 为 Ultra
  时选择 `MultiAgentMode::Proactive`：
  [`multi_agents.rs#L39-L55`](https://github.com/openai/codex/blob/d6ea5991e7f91eec5bad1fb4cb36f6be638878bc/codex-rs/core/src/session/multi_agents.rs#L39-L55)。
- session 会用模型 catalog 的 `model_info.multi_agent_version` 选择 runtime：
  [`session/mod.rs#L3249-L3260`](https://github.com/openai/codex/blob/d6ea5991e7f91eec5bad1fb4cb36f6be638878bc/codex-rs/core/src/session/mod.rs#L3249-L3260)。
- 若模型没有该字段，才回退到 feature 配置；普通回退可能只是 V1 或 Disabled：
  [`config/mod.rs#L1456-L1473`](https://github.com/openai/codex/blob/d6ea5991e7f91eec5bad1fb4cb36f6be638878bc/codex-rs/core/src/config/mod.rs#L1456-L1473)。

CC Switch v3.18.0/main 的两个静态模板、catalog 生成代码以及 PR #5265
均未出现 `multi_agent_version`。PR 单测也只断言 reasoning level 列表。
所以该 PR 是“Ultra picker 支持”，尚不是可以由源码证明的“完整主动
subagent Ultra 支持”。

## 请求层如何处理 Ultra

CC Switch 当前三条路径都没有把字面值 `ultra` 变成上游 effort：

1. 原生 Responses 路径把 Codex 请求交给 forwarder 后直接处理响应；除特定
   xAI namespace 工具兼容外，没有 Ultra effort 改写：
   [`handlers.rs#L808-L895`](https://github.com/farion1231/cc-switch/blob/708b38791cec070e634b2364958d6afba9000770/src-tauri/src/proxy/handlers.rs#L808-L895)。
2. Responses 转 Chat 的 effort mapper 只识别到 `max`，未知值返回 `None`：
   [`transform_codex_chat.rs#L458-L493`](https://github.com/farion1231/cc-switch/blob/708b38791cec070e634b2364958d6afba9000770/src-tauri/src/proxy/providers/transform_codex_chat.rs#L458-L493)。
3. Responses 转 Anthropic 只把 `xhigh | max` 映射到最高 thinking；
   `ultra` 不匹配：
   [`transform_codex_anthropic.rs#L31-L53`](https://github.com/farion1231/cc-switch/blob/708b38791cec070e634b2364958d6afba9000770/src-tauri/src/proxy/providers/transform_codex_anthropic.rs#L31-L53)。

这是正确的职责划分。OpenAI Codex 自己在构造请求时执行
`Ultra -> Max`：
[`client.rs#L176-L180`](https://github.com/openai/codex/blob/d6ea5991e7f91eec5bad1fb4cb36f6be638878bc/codex-rs/core/src/client.rs#L176-L180)，
并在实际 Responses reasoning 字段构造时调用该函数：
[`client.rs#L819-L830`](https://github.com/openai/codex/blob/d6ea5991e7f91eec5bad1fb4cb36f6be638878bc/codex-rs/core/src/client.rs#L819-L830)。

所以原生 Responses 第三方上游实际应收到 `max`，而不是 `ultra`，同时主动
subagent 是 Codex 本地行为。CC Switch 的 Chat/Anthropic 转换器不识别
字面值 Ultra 并不影响正常 Codex 请求；只有非 Codex 调用方直接向 CC Switch
传 `ultra` 时才会丢失 reasoning。

## 对第三方工具的准确判断

| 场景 | 能否使用 Codex Ultra |
|---|---|
| CC Switch v3.18.0 正常 Model Mapping | 不能显示/选择 Ultra |
| CC Switch 当前 main | 不能显示/选择 Ultra |
| PR #5265，已知 `gpt-5.6-sol/terra` 第三方模型 | 能显示/选择 Ultra；未设置 V2，不能确认完整主动 subagent |
| 自己维护完整 catalog，声明 Ultra + `multi_agent_version: "v2"` | 可以走完整客户端 Ultra 语义 |
| 直接向第三方 Responses API 发送 `"effort":"ultra"` | 不应这样做；Ultra 不是第三方上游 effort |

CC Switch 源码也明确区分自己生成的 catalog 和用户管理的外部 catalog，
不会把外部完整结构反向降级成简化 Model Mapping：
[`codex_config.rs#L1108-L1171`](https://github.com/farion1231/cc-switch/blob/708b38791cec070e634b2364958d6afba9000770/src-tauri/src/codex_config.rs#L1108-L1171)。
这提供了正式版中的手工绕行路径，但不是 CC Switch UI 原生支持。

## 对 codex-mixin 的建议

CC Switch PR #5265 验证了 picker 元数据的最小实现方向；Codex 当前手册进一步把
Ultra 定义为客户端的 multi-agent 模式，而不是 GPT-5.6 Sol/Terra 专属的远端
reasoning effort。因此 codex-mixin 采用更通用的实现：

1. 不必更换整份模板。
2. 所有自定义模型声明 `ultra` 和 `multi_agent_version: "v2"`。
3. 支持 thinking 或能力未知的模型暴露完整档位；明确不支持 thinking 的模型只保留
   `none` 和 `ultra`，让客户端仍可主动派生 subagent。
4. 网关把客户端专用的 `ultra` 映射为远端 `max`；明确不支持 thinking 时移除
   `reasoning`，不向 OneAPI 发送非法字面值。
5. catalog、协议转换和真实 OneAPI 测试同时覆盖非 GPT 模型的 Ultra。

OpenAI Codex 源码已经确认当前实现发给上游的是 `max`，不是 `xhigh`。
后续 Codex Desktop 端到端验证重点应放在：任意第三方模型选择 Ultra 后，网关收到
`reasoning.effort = "max"`（或对不支持 thinking 的模型不收到 `reasoning`），且
Codex 主动调用 `spawn_agent`。
