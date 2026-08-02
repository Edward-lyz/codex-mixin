<!-- codex-mixin:zh-Hans:start -->
## v0.3.13

### 修复每一轮都丢失 prompt 缓存

Codex 每轮都会往历史里追加一条 developer 消息，例如约 4.8 KiB 的 `<workspace_context>`（内含当前 git status 与目录树）；使用 steer 打断时还会额外追加一条 `<turn_aborted>`。此前网关会把历史里**所有** developer 消息抬进 Anthropic 的 `system` 字段，于是这些字节落在整部对话之前，导致每一轮的前缀缓存全部失效。

用一段 785 项的真实会话重放两轮，代价一目了然。修复前：

```
prefix_state=system_changed  changed_regions=system
reused_turns=0  message_prefix_turns=521  system_blocks=22 -> 23
```

修复后：

```
prefix_state=tail_rewritten  changed_regions=""
reused_turns=520  reused_bytes=1522360  system_blocks=9
```

- 现在只有请求开头连续的 developer/system 消息算系统提示；后面出现的留在原位映射成 user 消息（Anthropic 没有 developer role）。这也是工作区快照本该出现的位置，而不是把十几份互相矛盾的 git 状态堆在 prompt 顶部。
- Fusion 同样受益：judge 分析此前以 developer 消息追加，每轮都会改写 final 模型的 system，而那段文本本身就标注为不可信的参考上下文，放在 system 并不合适。
- 升级后现有会话会经历一次 cache reset，之后稳定命中。

### 缓存诊断

- 比对不再在第一个差异区域短路，改为报告全部变化区域：新增 `changed_regions` 与 `message_prefix_turns`，可区分「消息序列干净、只是 system 漂移」和「历史本身被改写」。
- `system` 改为逐 block 摘要，日志用 `system_prefix_blocks` / `system_blocks` 直接指出是第几块发生变化。
- 端到端回归新增「追加 developer 消息」场景，并断言四轮的 `changed_regions` 全为空。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.13

### 修正每一輪都遺失 prompt 快取

Codex 每輪都會往歷史追加一條 developer 訊息，例如約 4.8 KiB 的 `<workspace_context>`（內含目前 git status 與目錄樹）；使用 steer 打斷時還會額外追加一條 `<turn_aborted>`。此前閘道會把歷史裡**所有** developer 訊息抬進 Anthropic 的 `system` 欄位，於是這些位元組落在整段對話之前，導致每一輪的前綴快取全部失效。

用一段 785 項的真實會話重放兩輪，代價一目了然。修正前：

```
prefix_state=system_changed  changed_regions=system
reused_turns=0  message_prefix_turns=521  system_blocks=22 -> 23
```

修正後：

```
prefix_state=tail_rewritten  changed_regions=""
reused_turns=520  reused_bytes=1522360  system_blocks=9
```

- 現在只有請求開頭連續的 developer/system 訊息算系統提示；後面出現的留在原位對應成 user 訊息（Anthropic 沒有 developer role）。這也是工作區快照本該出現的位置，而不是把十幾份互相矛盾的 git 狀態堆在 prompt 頂端。
- Fusion 同樣受益：judge 分析此前以 developer 訊息追加，每輪都會改寫 final 模型的 system，而那段文字本身就標註為不可信的參考脈絡，放在 system 並不合適。
- 升級後現有會話會經歷一次 cache reset，之後穩定命中。

### 快取診斷

- 比對不再於第一個差異區域短路，改為回報全部變化區域：新增 `changed_regions` 與 `message_prefix_turns`，可區分「訊息序列乾淨、只是 system 漂移」與「歷史本身被改寫」。
- `system` 改為逐 block 摘要，日誌用 `system_prefix_blocks` / `system_blocks` 直接指出是第幾塊發生變化。
- 端到端回歸測試新增「追加 developer 訊息」情境，並斷言四輪的 `changed_regions` 全為空。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.13

### Fixed losing the prompt cache on every turn

Codex appends a developer message to the history every turn, such as a 4.8 KiB `<workspace_context>` carrying the current git status and directory tree, plus a `<turn_aborted>` block whenever a turn is steered. The gateway used to lift **every** developer message in the history into the Anthropic `system` field, so those bytes landed ahead of the entire transcript and invalidated the prefix cache on every single turn.

Replaying two turns of a real 785-item session shows the cost. Before:

```
prefix_state=system_changed  changed_regions=system
reused_turns=0  message_prefix_turns=521  system_blocks=22 -> 23
```

After:

```
prefix_state=tail_rewritten  changed_regions=""
reused_turns=520  reused_bytes=1522360  system_blocks=9
```

- Only the leading run of developer or system messages counts as a system prompt now. Later ones stay where they happened, as user turns, because Anthropic has no developer role. That is also the correct position for a workspace snapshot, rather than stacking a dozen contradictory git states at the top of the prompt.
- Fusion benefits the same way: its judge analysis was appended as a developer message and rewrote the final model's system prompt every turn, even though the text explicitly labels itself untrusted advisory context.
- Existing sessions take one cache reset on upgrade and hit consistently afterwards.

### Cache diagnostics

- The comparison no longer short-circuits on the first differing region. Reports now carry `changed_regions` and `message_prefix_turns`, which separates "the message sequence is clean and only the system prompt drifted" from "the history itself was rewritten".
- `system` is digested per block, so `system_prefix_blocks` and `system_blocks` point straight at the block that moved.
- The end-to-end regression gained an appended-developer-message case and asserts that `changed_regions` is empty on all four turns.
<!-- codex-mixin:en:end -->
