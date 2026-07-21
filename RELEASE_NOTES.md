<!-- codex-mixin:zh-Hans:start -->
## Hotfix

- 修复保存或修改 Fusion profile 后，Codex 本地模型目录没有重新生成，导致 `mixin/fusion/<id>` 不出现在模型选择器的问题。
- `refresh-codex-catalog` 现在会根据当前上游模型与 Fusion 配置重建完整 catalog，而不再只更新已有模型的 Web Search 标记。
- 更新后重新启动 Codex Mixin 和 Codex App，即可加载 Fusion 虚拟模型。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## Hotfix

- 修正儲存或修改 Fusion profile 後，Codex 本機模型目錄沒有重新產生，導致 `mixin/fusion/<id>` 未出現在模型選擇器的問題。
- `refresh-codex-catalog` 現在會依照目前的上游模型與 Fusion 設定重建完整 catalog，而不再只更新既有模型的 Web Search 標記。
- 更新後重新啟動 Codex Mixin 與 Codex App，即可載入 Fusion 虛擬模型。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## Hotfix

- Fixed an issue where saving or editing a Fusion profile did not rebuild Codex's local model catalog, so `mixin/fusion/<id>` was missing from the model picker.
- `refresh-codex-catalog` now regenerates the complete catalog from the current upstream models and Fusion profiles instead of only updating Web Search flags on existing entries.
- Restart Codex Mixin and the Codex App after updating to load the Fusion virtual models.
<!-- codex-mixin:en:end -->
