# Fusion 机制实施计划

给 codex-mixin 加入 OpenRouter 式 fusion：多个 panel 模型并行分析 → judge 模型结构化对比 → final 模型流式产出最终回答。用户通过虚拟模型 `mixin/fusion/<profile-id>` 在 Codex 模型选择器中直接选用。

## 核心设计决策（已定，不要偏离）

1. **三段式管线**：Panel(N 并行) → Judge(严格 JSON) → Final(流式 + 完整 Codex 工具)。成本 N+2 次调用。
2. **每个新用户轮次都 fusion**：不区分 Plan / Default / 写代码阶段；只要 input 最后一个可操作项是 user 消息，就触发 Panel→Judge。`function_call_output` 等同一轮工具续跑直接改写 model 为 `final_model` 透传，避免重复执行 Panel。旧配置中的 `fuse_every_user_turn` 字段仅为兼容保留，保存时固定为 true。
3. **不做 ReadOnlyToolBroker / 跨请求状态机**：Panel 的只读工具在 gateway 进程内直接执行（本机运行、用户自己的权限），整个 Panel 工具循环在一次请求处理内完成。不往返 Codex 客户端。
4. **Final 阶段就是普通代理请求**：把 Judge 分析注入 input、model 改写为 final_model，走现有转发路径。工具续跑（input 最后可操作项为 `function_call_output`）由用户轮次谓词排除，不需要 pending_tools 映射。
5. **model 字段一致性**：返回给下游的所有事件中 model 保持 `mixin/fusion/<id>` 原样，仅发上游时改写。否则 Codex 的 `previous_response_id` 校验会失败。
6. **Panel/Judge 走非流式收口**：复用现有 SSE 映射，收集到 `response.completed` 取全文，不另写非流式 provider 实现。

## TODO

### 1. `src/upstream.rs`（新文件）：抽出上游调用公共层

- [x] 现状：HTTP `responses()`（`src/server.rs` ~line 643）和 WS `proxy_custom_responses_ws()`（~line 1198）各写了一遍 custom 上游逻辑（Anthropic/OpenAiChat 两种 UpstreamKind 的转换 + 请求 + SSE 映射）。抽成：
  - `stream_response(state, body) -> impl Stream<Item = ResponsesEvent SSE bytes>`：现有流式路径。
  - `collect_response(state, body) -> CollectedResponse`：内部消费 `stream_response`，取 `response.completed` 里的 output text / usage，供 Panel/Judge 用。
- [x] `responses()` 与 WS 分支改为调用公共层，行为不变（回归现有测试）。

### 2. `src/fusion.rs`（新文件）：FusionProfile + FusionEngine

- [x] `FusionProfile` 结构：
  ```json
  {
    "id": "default",
    "panel_models": ["model-a", "model-b", "model-c"],
    "judge_model": "model-d",
    "final_model": "model-d",
    "min_successful": 2,
    "max_completion_tokens": 2048,
    "timeout_ms": 90000,
    "fuse_every_user_turn": true,
    "panel_tools": { "enabled": true, "max_rounds": 4, "max_calls_per_model": 8 }
  }
  ```
- [x] 持久化：`StoredGatewayConfig`（`src/config.rs` ~line 415）加 `fusion_profiles: Vec<FusionProfile>` 字段（serde default，向后兼容旧配置文件）。`GatewayConfig` 带解析后的 profiles。
- [x] 校验：panel 1–8 个；panel/judge/final 不得引用 `mixin/fusion/` 前缀（防递归）；`min_successful <= panel 数`。
- [x] 路由：`ModelRoute { Official, Direct, Fusion { profile_id } }` 解析函数；`mixin/fusion/<id>` 命中 Fusion。
- [x] 用户轮次判定谓词 `should_fuse_turn(body) -> bool`：以 input 最后可操作项区分新 user 消息与同一轮 `function_call_output`，不受历史 assistant / `previous_response_id` 限制。
- [x] Panel 阶段：
  - 从 input 提取用户任务与 `<environment_context>` 中的 cwd（工具 jail 根目录；解析不到则禁用工具，Panel 退化为纯文本）。
  - Panel prompt 精简：用户任务 + 必要上下文，不带 Codex 全量 instructions。
  - `FuturesUnordered` 并行，每个 panel 带只读工具做进程内工具循环（见 §3），单模型超时 `timeout_ms`。
  - 失败降级：成功数 >= `min_successful` 继续；不足则跳过 Judge，直接透传原请求给 final_model（记 warning）。
- [x] Judge 阶段：`collect_response` 非流式；prompt 要求输出严格 JSON：
  ```json
  { "consensus": [], "contradictions": [], "partial_coverage": [], "unique_insights": [], "blind_spots": [], "recommended_approach": [] }
  ```
  Panel 输出作为不可信数据（分隔符包裹，指令注入防护措辞）。JSON 解析失败时容忍：整段文本作为分析注入 Final。
- [x] Final 阶段：原始请求 body 深拷贝，input 末尾注入一条 developer/user 消息（Judge 分析 + 各 panel 要点摘要），model 改写 final_model，走 `stream_response` 原样转发给下游。
- [x] 进度事件：Panel/Judge 期间向下游合成 `response.reasoning_summary_text.delta` SSE 事件（"panel <model> 完成 (2/3)…"、"judge 分析中…"），避免 Codex 客户端干等假死。注意事件序号/结构与 Codex 期望一致（参考 `src/openai_events.rs` 现有事件构造）。

### 3. `src/fusion_tools.rs`（新文件）：Panel 进程内只读工具

- [x] 工具集（OpenAI function-calling schema，注入 Panel 请求）：
  - `read_file { path, offset?, limit? }`：std::fs 读，单结果 32KB 截断。
  - `list_files { path?, glob? }`：walkdir，限制条目数。
  - `grep { pattern, path?, glob? }`：优先调用系统 `rg`，无则退化为内置逐文件正则；只读。
  - `git_inspect { subcommand, args? }`：仅白名单 `status` / `log` / `diff` / `show`，参数过滤（禁止 `-o`、`--output` 等写出参数）。
- [x] 安全约束：所有路径 canonicalize 后必须位于 cwd（jail 根）内，拒绝 symlink 逃逸；拒绝任意 shell；每模型上限 `max_rounds` 轮 / `max_calls_per_model` 次调用，超限后强制无工具收尾。
- [x] 工具循环：panel 模型返回 tool_calls → 进程内执行 → 结果作为 tool role 消息追加 → 再调 `collect_response`，直到无 tool_calls 或超限。

### 4. 暴露虚拟模型

- [x] `src/catalog.rs`（catalog 生成 ~line 69）与 `/v1/models`：为每个 profile 追加 `mixin/fusion/<id>` 条目，display_name 形如 `Fusion (<id>): a+b+c → judge d`，description 说明管线。使 Codex 模型选择器可直接选中。
- [x] `normalize_custom_model_alias` / 官方转发判定不得把 fusion slug 误路由。

### 5. 接入两个入口

- [x] HTTP `responses()`：model 命中 Fusion 且为新用户轮次 → FusionEngine；工具结果续跑 → 改 model 透传。
- [x] WS custom 分支（`proxy_custom_responses_ws` / 路由 helpers ~line 1701）：同上，复用同一 FusionEngine。
- [x] 下游事件中回填 model 为 fusion slug。

### 6. macOS App UI：Fusion 配置入口（必做，用户在此选择参与 fusion 的模型）

- [x] `macos/MenuBarApp.swift`：菜单栏新增 "Fusion 设置…" 菜单项（放在 Provider 设置附近，~line 450 的 action 区域）。
- [x] 新建 `macos/FusionSettingsWindow.swift`（参考 `ModelBenchmarkWindow.swift` 的窗口控制器写法）：
  - 启动时经网关 `/v1/models` 拉取可用模型列表（带 gateway key，同 benchmark 窗口 ~line 542 的做法）；网关未运行时提示先启动。
  - Panel 模型：checkbox 列表多选（1–8 个，超出/为空时禁用保存并提示）。
  - Judge / Final 模型：两个 NSPopUpButton 单选（默认同一个模型）。
  - 高级选项（次要区域）：`min_successful`、`timeout_ms`、每个用户轮次固定启用的说明、panel 工具开关。
  - Profile id 文本框（默认 `default`）；MVP 先支持编辑单个 profile，多 profile 列表增删可后续加。
- [x] 持久化：直接读写 stored config JSON 中的 `fusion_profiles` 数组（复用 `loadStoredConfig` ~line 1622 及与登录设置面板相同的保存机制），保存后沿用现有"设置保存→重启网关进程"逻辑使 catalog 生效。
- [x] 保存前本地校验与 Rust 端一致：1–8 panel、禁止 `mixin/fusion/` 前缀、`min_successful <= panel 数`。
- [x] `macos/build_app.sh` 若按文件列表编译需加入新 Swift 文件。

### 7. 测试（`tests/gateway_http.rs` 风格，参考现有 mock 上游）

- [x] profile 校验：1–8 panel、递归引用拒绝、min_successful 越界。
- [x] 首轮触发 fusion、次轮（含 assistant 历史 / function_call_output / previous_response_id）直接透传。
- [x] Panel 并行（mock 两个慢上游，总耗时 < 串行和）。
- [x] 部分 panel 失败降级、全失败回退纯 final。
- [x] Judge JSON 解析失败容忍。
- [x] 只读工具：路径逃逸拒绝、git 子命令白名单、轮次上限。
- [x] SSE 中 model 字段保持 fusion slug；进度事件合法。
- [x] `cargo fmt` / `cargo clippy` / `cargo test` 全绿。

### 8. Plan 后续写代码轮次

- [x] Fusion 不读取 collaboration mode；Plan、Default 与写代码阶段的新 user 消息使用同一路由。
- [x] 有历史 assistant 或 `previous_response_id` 的后续 user 消息仍执行 Panel→Judge→Final。
- [x] 同一轮 `function_call_output` 续跑不重复 Fusion，直接交给 `final_model`。

### 9. 暂缓（本次不做）

- profiles 管理 REST API（GET/PUT/DELETE /v1/fusion/profiles）与 CLI 子命令（App UI 直接读写 stored config，暂不需要）。
- 多上游（跨 provider 不同 URL/key）；当前单上游即可，FusionEngine 保持 provider 无关，未来只改模型解析层。
