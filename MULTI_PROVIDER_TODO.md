# 多 Provider、模型筛选与 OpenCode Go TODO

目标：Codex Mixin 同时管理多个 Provider。每个 Provider 独立保存协议、地址、密钥、模型发现、模型 allowlist、额度和生图配置；Codex 通过带 Provider 后缀的模型 slug 路由到正确上游。

OpenCode Go 是首个新增预设，但不得写供应商专用推理分支。

## cc-switch 参照结论

已核对 [`farion1231/cc-switch`](https://github.com/farion1231/cc-switch) 的 `a377d79303bc1e592d2783d559ca5bd6b8ba1417`：

- OpenCode Go 没有专用 OAuth/订阅交换流程，使用普通 Bearer API Key。
- Codex 预设是 `https://opencode.ai/zen/go/v1` + `openai_chat`。本项目拆成根地址 `https://opencode.ai/zen/go` 与 `/v1/chat/completions`，最终请求 URL 等价。
- cc-switch 内置 `glm-5.2`、`glm-5.1`、`kimi-k2.7-code`、`deepseek-v4-pro`、`deepseek-v4-flash`、`mimo-v2.5-pro` 六个模型。本项目用它们作为首次可用的静态种子，同时保留 `/v1/models` 在线刷新；刷新失败继续使用种子/上次缓存。
- cc-switch 的通用模型发现会按 base URL 形态推导 `/models`，并允许显式 models URL 覆盖。Codex Mixin 不猜路径，预设给精确路径，自定义 Provider 要求用户明确配置。

## 核心决策

1. 不兼容旧的单 Provider 配置结构，也不保留旧的无后缀第三方模型 ID；切换时一次性更新配置、Catalog、Fusion 配置和测速缓存版本。
2. OpenAI 官方模型继续使用原始 ID；所有第三方模型统一生成 `<upstream-model-id>-<provider-id>`。
3. 后缀只用于展示和请求入口。Registry 必须建立完整 slug 到 `(provider_id, upstream_model_id)` 的精确索引，禁止通过 `rsplit('-')` 猜 Provider。
4. 不同 `(provider, model)` 生成相同 slug，或第三方 slug 与官方/Fusion 模型冲突时拒绝启动并指出冲突项，不能静默覆盖。
5. 首版支持 Anthropic Messages、OpenAI Chat Completions、OpenAI Responses 三种上游协议。
6. 每个 Provider 保存显式 `selected_models` allowlist；Catalog、Fusion 和测速只使用已启用 Provider 中已选择且当前可用的模型。
7. 首次发现模型默认全选；后续新增模型默认不选。刷新失败沿用缓存；刷新成功但模型消失时暂时隐藏，保留选择供恢复后重新出现。
8. Provider ID 创建后不可修改，只允许小写字母、数字、点、下划线和短横线；`official`、`mixin` 为保留值。
9. 一个模型 slug 永远确定性路由到一个 Provider；首版不做同模型负载均衡、自动跨 Provider 重试或故障转移。
10. “可接入所有模型”限定为上游符合三种受支持协议之一，且其流式事件、工具调用、图片、structured output、reasoning 和 usage 语义能被声明或适配；不能仅凭接口路径宣称完全兼容。
11. 配置只允许通过 Rust CLI/配置服务原子修改。macOS App 不再直接读写含明文密钥的 `config.json`，避免并发覆盖、绕过校验和密钥泄漏。

## 功能入口审计矩阵

| 功能入口 | 当前单 Provider 假设 | 多 Provider 改造 |
| --- | --- | --- |
| macOS Provider 设置 | 一个下拉框和一套 URL/Key | Provider 列表、独立详情、模型复选和启停 |
| CLI `login/logout/config` | 写一组全局 upstream 字段 | Provider CRUD；config/status 返回脱敏数组 |
| 配置文件与环境变量 | 顶层字段和单组 `CODEX_GATEWAY_UPSTREAM_*` | 配置文件是 Provider 数组 SSOT；运行行为不接受环境变量覆盖，只有 HOME/PATH/测试配置路径等资源定位仍使用环境 |
| 网关启动与自启 | 缺任一全局上游就不能启动 | 整体校验 Registry；单 Provider 发现失败可降级 |
| `/v1/models` 与 Catalog | 请求一个 `/models` 并统一补 metadata | 并发聚合各 Provider 已选模型并统一加后缀 |
| Codex 安装/刷新 | Catalog 只接收一个 Provider suffix | 直接写聚合 Catalog，默认模型使用完整 slug |
| 请求鉴权分类 | 根据预设后缀判断 official/custom | auth、HTTP、WS 共用同一个精确 `ResolvedModelRoute` |
| HTTP/WS Responses | custom 分支固定一套 URL/协议/Key | 每个请求先查 slug 索引，再取得 Provider runtime |
| WS history | 只校验模型名 | 同时绑定完整 slug、Provider 和原始模型 |
| Fusion | Provider 前缀最终仍落到同一全局上游 | Panel/Judge/Final 分别解析并调用自己的 Provider |
| imagegen | route 只记录“走自定义上游” | route 必须记录具体 Provider，使用对应生图配置 |
| 模型测速 | 一个 config 跑全部模型、一次 quota 差值 | target 绑定 Provider；分组调度、分组额度和币种 |
| 额度菜单与 CLI | 只显示一个 quota | 按 Provider 查询和展示，局部失败不影响其他项 |
| web search 探测 | cache identity 是唯一全局上游 | cache key 增加 Provider，探测请求按 Provider 发送 |
| metadata/cache | 仅用模型 ID 匹配 | 使用 Provider + 原始模型，Catalog 输出再附后缀 |
| `/healthz` 与状态轮询 | 只表示进程活着、菜单只看一个上游 | 保留轻量进程存活；新增 Provider 级 readiness/错误摘要 |
| 配置保存/Fusion 保存 | Swift 可直接覆盖 JSON | 统一走带锁、校验、原子替换的 CLI mutation |
| 日志/doctor/real tests | 默认错误来自唯一上游 | 所有诊断携带 Provider ID且密钥脱敏 |
| 更新/卸载/历史 | 管理一个本地 gateway provider | 仍保持一个本地网关进程；不按上游拆实例 |

### 入口分层与唯一责任

多 Provider 改造按入口分成五层。任何入口不得越过上一层直接从配置猜模型归属：

1. **配置入口**：CLI、macOS 设置、Fusion 设置只修改 `StoredGatewayConfig`，经统一 mutation 完成文件锁、版本校验、引用校验、`0600` 临时文件和原子替换。
2. **发现入口**：启动预热、`providers discover`、设置页刷新模型只更新目标 Provider 的缓存和 allowlist；刷新结果通过一次 Registry rebuild 发布。
3. **解析入口**：HTTP、WS、Fusion、imagegen、benchmark、web-search probe 全部调用 `resolve_model_route`，一次解析得到不可变的 `ResolvedProviderModel`。
4. **协议入口**：Anthropic Messages、OpenAI Chat、OpenAI Responses adapter 只接收 `ProviderRuntime + upstream_model_id`，不能接收全局配置或自行处理后缀。
5. **展示入口**：Catalog、CLI JSON、菜单栏、设置页、测速页只消费脱敏 view model；不得读取含明文 Key 的 stored config。

### 具体 API/命令/UI 入口

| 表面入口 | 多 Provider 输入 | 输出/持久状态 |
| --- | --- | --- |
| `GET /v1/models` | Registry snapshot | 稳定排序的已启用、已选择、当前可用 catalog slug |
| `GET /v1/codex-model-catalog` | 聚合模型 + Provider/模型能力 | catalog slug；缓存版本必须包含 Registry generation |
| `POST /v1/responses` | 入站 catalog slug | 上游原始 model；下游事件恢复 catalog slug |
| `GET /v1/responses` WebSocket | 每帧 catalog slug + previous response | history 绑定 route identity，跨 Provider continuation 返回 4xx 风格失败事件 |
| `POST /v1/images/generations` | image route marker | marker 绑定 Provider ID，使用该 Provider 图片 URL/auth |
| `POST /v1/images/edits` | image route marker | 自定义 Provider 仍拒绝编辑；无 marker 才走官方 |
| `GET/POST /v1/model-benchmarks` | Provider/model filters | Provider 分组结果、分币种费用、snapshot schema version |
| `healthz/status/doctor` | Registry readiness | 进程健康与 Provider 健康分开；局部失败为 degraded |
| `providers ...` CLI | Provider ID | 脱敏 CRUD/discover/select/test；修改后统一校验与失效 |
| `models/quota/probe-web-search` CLI | 可选 Provider filter | 数组结果；单 Provider 失败不终止其他 Provider |
| `catalog/install-codex/refresh-codex-catalog` | 聚合 Catalog | 默认模型始终是完整 catalog slug |
| macOS 设置/菜单/测速/Fusion | CLI JSON 或 gateway JSON | Provider 列、筛选、分组状态；Swift 不直接覆盖 config |

### 持久状态与缓存键审计

| 状态 | 旧键 | 新键/失效条件 |
| --- | --- | --- |
| 模型发现缓存 | 全局 models | `provider_id`；目标 Provider 配置、发现结果变化时失效 |
| Catalog response cache | TTL | `registry_generation + metadata_generation + web_search_generation` |
| web-search capability | model ID | `ProviderModelKey`；Provider/model 配置变化时失效 |
| WS continuation | response ID + model | `route identity + response ID`；禁止跨 Provider/Fusion/official 续接 |
| imagegen route | route ID -> expiry | route ID -> `{provider_id, expires_at}` |
| benchmark snapshot | model ID | schema v2 + catalog slug + ProviderModelKey |
| quota before/after | 单一数值 | `provider_id -> {currency, before, after}`；不同币种绝不求和 |
| Fusion 引用 | provider 前缀/裸模型 | `official:<model>` 或完整 catalog slug |
| metadata/capability | canonical model | 默认值可按 upstream ID复用，Provider override 必须以 `ProviderModelKey` 覆盖 |
| 日志/错误上下文 | model | `provider_id + catalog_slug + upstream_model_id`，统一密钥脱敏 |

### 配置变更的联动入口

Provider 的 add/update/enable/disable/remove/discover/select 不能只改 JSON。一次成功 mutation 必须按顺序完成：校验 Fusion 与默认模型引用、原子保存、重建 Registry、失效模型/Catalog/web-search cache、刷新托管 Codex Catalog、重启或热替换网关 snapshot。任何一步失败都要保留旧配置可恢复副本，并报告具体 Provider。

## 模型身份与路由边界

| 边界 | 必须使用的身份 |
| --- | --- |
| Codex Catalog、设置 UI、HTTP/WS 入站 | `catalog_slug`，例如 `glm-5.2-opencode-go` |
| Registry 主键 | 完整 `catalog_slug -> ProviderModelKey` 映射 |
| Provider runtime、cache、metadata、probe、benchmark | `ProviderModelKey { provider_id, upstream_model_id }` |
| 发给上游的 JSON body | 原始 `upstream_model_id`，例如 `glm-5.2` |
| 返回 Codex 的 response/event/history | 恢复原 `catalog_slug` |
| 官方模型 | 原始官方 slug，并由官方 Catalog 精确集合判定 |
| Fusion 虚拟模型 | 保留 `mixin/fusion/<id>` 命名空间 |

禁止在 Catalog 生成之后通过截字符串恢复 Provider。任何需要 Provider 的功能入口都必须持有 `ResolvedProviderModel` 或 `ProviderModelKey`。

## 新配置形态

最终删除顶层单 Provider 字段，改为：

```json
{
  "config_version": 2,
  "gateway_bind": "127.0.0.1:8787",
  "gateway_api_key": "...",
  "providers": [
    {
      "id": "opencode-go",
      "display_name": "OpenCode Go",
      "enabled": true,
      "preset_id": "opencode-go",
      "protocol": "open_ai_chat",
      "base_url": "https://opencode.ai/zen/go",
      "api_path": "/v1/chat/completions",
      "model_source": {
        "kind": "open_ai_compatible",
        "path": "/v1/models"
      },
      "auth": {
        "header": "authorization_bearer",
        "api_key": "..."
      },
      "selected_models": [
        "glm-5.2",
        "deepseek-v4-flash"
      ],
      "cached_models": [
        {
          "id": "glm-5.2",
          "display_name": "GLM 5.2",
          "context_window": 204800
        }
      ],
      "models_refreshed_at_ms": null
    }
  ],
  "fusion_profiles": []
}
```

配置文件继续以 `0600` 保存。Provider 密钥不得出现在日志、status 文本、测速快照或 HTTP 错误里。

## 1. Provider Registry 与预设

- [x] 定义 `ProviderDefinition`、协议、认证、模型发现、缓存模型和 Registry 基础类型。
- [x] 定义模型后缀生成与精确索引；覆盖 Provider/模型含短横线和上游模型含 `/` 的情况。
- [x] 固化 OpenCode Go 预设：根地址 `https://opencode.ai/zen/go`，Chat `/v1/chat/completions`，Models `/v1/models`，Bearer API Key，并用 cc-switch 的六模型 Catalog 作为初始缓存和默认选择。
- [x] 在 stored config 中引入版本化 `providers` 数组，并完成运行路径切换。
- [x] 用 `providers: Vec<ProviderDefinition>` 替换 `StoredGatewayConfig` / `GatewayConfig` 的全局单 Provider 字段。
- [x] 将现有 Custom、Baidu OneAPI、OpenRouter、DeepSeek 改成生成 `ProviderDefinition` 的预设模板。
- [x] 定义只读 `ProviderRuntime`：预先解析 URL、认证、协议适配器、quota 和请求策略；推理、发现、额度和生图入口统一消费 runtime。
- [x] 删除 Provider 和全局运行行为的环境变量覆盖；Provider、bind 和网关鉴权只从版本化配置加载，临时 bind 仅接受显式 CLI 参数。
- [ ] 启动时整体校验 Provider ID、重复 Provider、URL/path、认证、重复模型、Provider/官方/Fusion slug 冲突和 Fusion 引用；任一结构错误拒绝启动。
- [ ] 为自定义 Provider 预留受控的额外 header/query/body 参数，不允许把供应商特例写成散落的 `match provider_id`；日志统一对所有 Provider 密钥做替换脱敏。
- [ ] 模型能力进入 Provider/模型描述：工具、并行工具、图片输入、structured output、reasoning、service tier、context window；Catalog 不能宣传适配器无法兑现的能力。

## 2. 模型发现、选择与 Catalog

- [ ] 为每个启用 Provider 建立独立模型缓存、刷新时间和最近错误；并发刷新时故障隔离。
- [x] 支持 OpenAI-compatible GET、Baidu available-models POST 和静态模型列表三种发现方式。
- [x] OpenCode Go 首次使用内置六模型种子；在线刷新成功后按通用规则更新，失败时不得把种子清空。
- [x] 首次成功发现时全选；之后以保存的 allowlist 为准，新模型只标记“新增”。
- [x] 整个刷新请求失败时保留上次缓存；成功响应原子替换缓存，使已下线模型从 Catalog 暂时消失。
- [ ] 发现结果去重、稳定排序，并保留 `selected_models` 的用户顺序；单个坏模型条目要记录 Provider 级 warning，不能污染其他 Provider。
- [x] `/v1/models`、`/v1/codex-model-catalog`、安装和刷新命令只聚合 `enabled && selected && available` 模型。
- [ ] metadata resolver 以 `(provider_id, upstream_model_id)` 输入生成带后缀 Catalog；display name 明确显示 Provider。
- [x] web-search capability cache 改为 Provider 维度，不能让同名模型共享探测结果。
- [ ] Provider discover/select/enable/disable/remove 后统一失效 models、Catalog、web-search 和 Fusion option cache；即使当前实现通过重启生效，也只能有一个失效入口。
- [x] 聚合结果顺序固定为 Provider 配置顺序 + 模型顺序，供默认模型选择、UI diff 和测试稳定使用。

## 3. 推理入口与协议转换

- [x] `AppState` 持有不可变的 Provider Registry snapshot 与每个 Provider 的 runtime/client/cache，不再从全局 `GatewayConfig` 取上游；一次请求全程使用同一 snapshot。
- [x] 提供唯一的 `resolve_model_route`，供 HTTP、WS、Fusion 和 image route 共用；禁止各入口自行判断后缀。
- [ ] HTTP `/v1/responses` 用精确官方/Fusion/Registry 集合判定路由，并取得 Provider 和原始模型。
- [x] WS `/v1/responses` 的 history key 同时包含完整 slug、Provider 和原始模型，拒绝跨 Provider 复用 `previous_response_id`，避免不同上游 response ID 撞车。
- [x] Anthropic Messages、OpenAI Chat 复用现有请求转换、工具映射和 SSE → Responses 映射。
- [x] 新增 OpenAI Responses 直通：请求前恢复原始模型 ID，返回事件中的 `model` 全部改回带后缀 slug。
- [ ] 所有协议的非流式响应、流式事件、usage、error envelope 和断流补偿都返回入站 `catalog_slug`，不得泄漏原始模型 ID造成后续 history 换路。
- [ ] OneAPI affinity、Anthropic version/beta、auth header、超时、User-Agent、自定义 header/query/body 和路径全部从目标 Provider runtime 读取。
- [ ] reasoning/tool/structured-output/image 字段按模型能力转换；不支持的能力在发请求前返回明确 4xx，不把确定性错误留给上游猜测。
- [x] 未知 slug、禁用 Provider、未选择模型和当前不可用模型分别返回可诊断的 4xx；上游失败保留 Provider ID但脱敏。
- [x] 不做隐式跨 Provider fallback；同一请求的重试只能留在原 Provider 且遵守幂等与协议规则。

## 4. Fusion

- [x] Fusion 设置窗口直接保存完整 catalog slug；官方模型仍保存 `official:<model>`。
- [x] Panel/Judge/Final 每次通过 Registry 解析自己的 Provider，可同时使用不同 URL、协议和密钥。
- [ ] Provider 或模型被禁用/取消选择/删除时，保存 Provider 配置前列出受影响 Fusion profile 并阻止产生悬空引用。
- [ ] Fusion 进度、详情和可视化显示 Provider 名；最终下游 `model` 仍保持 `mixin/fusion/<id>`。
- [x] Fusion profile 保存改走统一 CLI mutation 和 Registry 校验，删除 Swift 直接覆盖 `config.json` 的路径。
- [ ] Panel/Judge/Final 的错误、timeout、usage 和日志分别保留 Provider ID；一个阶段失败不能被错误归因到默认 Provider。

## 5. 图片生成

- [x] `ImageRouteRegistry` 的 route value 从过期时间扩展为 `{provider_id, expires_at}`。
- [x] 自定义模型触发 imagegen 时记录当前 Provider；随后 `/v1/images/generations` 使用对应 Provider 的 URL、Key 和认证头。
- [x] Provider 未配置生图时保持现有官方生图回退；自定义图片编辑仍明确拒绝，直到单独实现。
- [x] 同时请求多个 Provider 生图时路由不能串线，过期/未知 route 继续返回 4xx。
- [x] route marker 必须继续使用随机 route ID，而不是 prompt/model 后缀；解析成功后返回 `{clean_prompt, provider_id}`，并覆盖重复 prompt、并发和 replay 测试。

## 6. 模型测速

- [x] 测速输入改为 `BenchmarkTarget { catalog_slug, provider_id, upstream_model_id }`，每个 target 绑定自己的 Provider runtime。
- [x] `ModelBenchmarkResult` 增加 `provider_id`、`provider_name`、`upstream_model`；`model` 保存带后缀 Catalog slug。
- [x] 快照版本升级；启动参数允许按 Provider 和模型筛选，默认只测速所有已选择且可用的第三方模型。
- [x] 不迁移旧单 Provider 快照；发现旧版本时明确提示重新测速，不能把旧结果错误归到任一 Provider。
- [x] 调度按 Provider 分组：组内默认串行，Provider 之间有限并发，避免单个订阅被并发打爆又避免多 Provider 完全串行。
- [x] TTFT/TPS/token 解析按目标协议执行；OpenAI Responses 增加对应 usage/event 解析。
- [x] quota 配置增加解析器类型、认证方式和币种；before/after 必须按 Provider 分别采样，成本保存为 Provider 级汇总，禁止把不同币种相加成一个总数。
- [x] Provider 失败只标记该组剩余 target 失败或继续下一模型，不终止其他 Provider。
- [x] macOS 测速窗口增加 Provider 列、Provider 筛选和分组成本；持久化快照中断恢复逻辑适配多 Provider。
- [x] `POST /v1/model-benchmarks` 接受 provider/model filters，响应回显最终 target；运行中删除/禁用 Provider 通过不可变 runtime snapshot 完成本轮，下一轮再生效。

## 7. 额度、状态与诊断

- [x] `quota` 接受 `--provider <id>`；无参数返回所有已配置额度接口的 Provider 结果数组。
- [x] 菜单栏额度区域改成按 Provider 展开的列表，分别显示当前 CLI 可提供的币种、已用值和错误；上限/剩余等待 quota view schema 增补。
- [x] `status --json` / `config` / `doctor` 输出 Provider 数、启用状态、协议、模型缓存时间、选择数量、可路由数量和健康状态；Key 只输出 configured/missing/redacted。
- [ ] web-search probe、metadata refresh 和日志字段增加 Provider ID；清缓存支持单 Provider 或全部。
- [ ] 网关启动允许部分 Provider 的模型发现失败，只要配置结构合法且至少存在一个可路由模型。
- [x] `/healthz` 只检查进程存活，不在每次轮询打上游；Provider readiness 由缓存、allowlist、Key 和最近刷新错误派生，供 `status`/`doctor`/菜单使用并区分 healthy/degraded/disabled。
- [x] `quota --json` 固定返回 Provider 结果数组，单项包含 `provider_id/currency/value/error/stale_at`；当前每次实时查询、不复用旧值，因此 `stale_at` 固定为 `null`。
- [ ] 错误和 tracing 统一携带 `provider_id`、`catalog_slug`、`upstream_model_id`，但对所有 Provider key、Authorization、x-api-key 和带密钥 URL 做中央脱敏。

## 8. 配置与操作入口

- [x] CLI 用 `providers list|add|update|enable|disable|remove|discover|select|test` 替换单 Provider `login/logout`。
- [x] macOS“设置供应商与密钥”改为 Provider 列表 + 详情：新增、编辑、启停、删除、测试连接。
- [x] 详情页模型区支持刷新、搜索、全选/全不选，并按 Provider 保存 allowlist。
- [x] 详情页模型区补齐“已选/新增/不可用”过滤和新增模型标记；暂时不可用的已选模型仍可显式取消，普通保存不会静默移出 allowlist。
- [x] OpenCode Go 加入预设列表；API Key 页面使用 `https://opencode.ai/go`，首次显示内置六模型种子，并允许在线刷新。
- [x] App 通过 CLI 读取脱敏 Provider view；Key 字段只显示“已配置”，空值表示保留旧 Key。
- [x] 增加显式“清除 Key”动作、二次确认和后端 mutation；UI 提示先停用，CLI 校验再次拒绝清除启用中 Provider 的 Key。
- [x] Rust CLI 的 Provider/Fusion mutation 使用文件锁、内存中完整校验、临时文件 `0600` 和原子替换；macOS 不再直接读写 `config.json`。
- [x] `providers test` 对动态模型源验证 URL/auth/models 响应；静态模型源只做配置验证，不在保存或测试时自动产生推理费用。
- [x] 保存 Provider 配置后重启网关并刷新 Codex Catalog；删除最后一个 Provider 时安全停服并回到等待配置状态。
- [ ] 重启或 Catalog 刷新失败时提供显式回滚/恢复入口，避免已保存但未应用被 UI 描述成普通失败。
- [ ] 删除/禁用/取消模型选择前统一检查 Fusion 引用、Codex 当前默认模型和正在运行的 benchmark，并给出可执行的阻塞原因。

## 9. Codex 安装、历史与服务生命周期

- [x] `install-codex` / `refresh-codex-catalog` 从聚合 Catalog 选择默认模型，不再接收单 Provider suffix 参数。
- [x] `custom-only` 要求至少一个已选且可用模型；默认模型使用完整后缀 slug。
- [x] 删除 `strip_provider_suffix` 和 `ProviderPreset::strip_model_provider_suffix` 等启发式代码；安装、校验和 OAuth proxy 全部按聚合 Catalog/Registry 精确集合工作。
- [ ] 官方 OAuth proxy 路由只按官方 Catalog 精确集合判断；所有第三方模型因带 Provider 后缀而不会顶掉官方模型。
- [ ] 当前 Codex 默认模型仍存在时原样保留；失效时按“Provider 配置顺序 + 模型顺序”选择完整 slug，交互式入口显示变更，非交互入口输出明确日志。
- [x] 历史会话迁移继续只处理 `model_provider=codex-mixin`，不重写模型 slug。
- [x] 启动、自启、重启、更新、卸载和托管 Codex 配置保持单本地网关实例，不为每个 Provider 启动进程。
- [x] macOS 自启和缺配置判断不再匹配 `run login --key` 错误字符串；Provider 设置读取结构化 CLI JSON，最后一个 Provider 删除后安全停服并回到等待配置状态。

## 10. 测试与验收

- [x] Registry 单测：ID、保留字、重复 Provider、后缀冲突、allowlist、禁用与不可用模型。
- [ ] Catalog 冲突测试覆盖第三方 slug 与官方 slug、`official:` 和 `mixin/fusion/` 保留命名空间冲突。
- [x] 两个 mock Provider 暴露同名模型，验证 Catalog slug、HTTP、WS、Key、URL和原始上游 model 均正确。
- [ ] 鉴权测试覆盖官方 OAuth、gateway key、未知 slug 和不同 Provider key，确保 auth 与实际路由使用同一个解析结果。
- [ ] 三种协议分别覆盖文本、图片输入、工具调用、并行工具、structured output、usage、错误和断流。
- [ ] 模型刷新覆盖首次全选、新增不选、失败用缓存、确认下线隐藏、恢复重现和局部故障。
- [ ] Fusion 覆盖三阶段跨三个 Provider；生图覆盖并发 route；测速覆盖多协议、多币种、超时和局部失败。
- [ ] 配置测试已覆盖并发 mutation、原子替换、权限 `0600` 和主要输出脱敏；Key 显式清除、崩溃恢复与中央日志脱敏仍待补。
- [ ] 更新中英文 README、配置示例、CLI help、macOS 截图和 release notes。
- [x] `cargo fmt`、`cargo clippy --all-targets --all-features`、`cargo test`、macOS app build 全绿。

## 11. 发布兼容、自动检测与问题回收

- [x] Linux x86_64/aarch64 Release 和 `.deb` 改用 musl 静态二进制；CI 用 ELF 检查拒绝任何 `GLIBC_*` 依赖。
- [x] 网关运行配置不再读取环境变量覆盖；macOS App 启动 CLI 时主动清除遗留的 `CODEX_GATEWAY_*`、`ANTHROPIC_BASE_URL` 和 `ANTHROPIC_API_KEY`。
- [x] Baidu OneAPI 额度用户名在 CLI 校验和 App 新增/编辑界面中设为必填。
- [x] `doctor` 提供文本与 JSON 报告，检查配置、权限、Provider 非付费模型接口、网关、Codex 集成和日志；App 增加“自动检测...”和复制报告入口。
- [x] 网关日志记录脱敏后的实际 Provider 配置、完整错误链和 Provider/模型路由身份；App CLI 操作失败与自动检测报告也写入运行日志。

## 实施依赖顺序

1. 配置 v2、Provider runtime、Registry 和统一 `resolve_model_route`。
2. 模型发现/选择、聚合 `/v1/models` 与 Catalog；此时 Codex 能看到稳定带后缀模型。
3. HTTP/WS 三协议路由与 history；此时普通单模型对话端到端可用。
4. Fusion、imagegen、web-search、metadata 等持有模型身份的旁路功能。
5. benchmark/quota、macOS Provider UI、Codex 安装刷新、状态诊断和完整回归。
