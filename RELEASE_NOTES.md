<!-- codex-mixin:zh-Hans:start -->
## v0.3.11

### DUCC 安装紧急修复

- 修复 macOS GUI 环境找不到 Homebrew `zstd`，导致 DUCC `.tar.zst` 校验失败的问题；Terminal 预检与 App 解压现在使用一致的 PATH，找不到 zstd 时自动回退到 bzip2。
- DUCC 安装错误现在保留阶段、命令、退出码和原始 stderr，便于直接定位失败原因。
- 移除 DUCX 认证选项、托管安装路径、CLI 参数、运行时桥接及相关探针；Baidu OneAPI 仅保留关闭或 DUCC loopback。
- 旧配置中的 DUCX bridge 值升级后会安全降级为关闭，不再启动或查找 DUCX。
<!-- codex-mixin:zh-Hans:end -->

<!-- codex-mixin:zh-Hant:start -->
## v0.3.11

### DUCC 安裝緊急修正

- 修正 macOS GUI 環境找不到 Homebrew `zstd`，導致 DUCC `.tar.zst` 驗證失敗的問題；Terminal 預檢與 App 解壓縮現在使用一致的 PATH，找不到 zstd 時自動回退到 bzip2。
- DUCC 安裝錯誤現在保留階段、命令、結束碼與原始 stderr，便於直接定位失敗原因。
- 移除 DUCX 認證選項、託管安裝路徑、CLI 參數、執行階段橋接與相關探針；Baidu OneAPI 僅保留關閉或 DUCC loopback。
- 舊設定中的 DUCX bridge 值升級後會安全降級為關閉，不再啟動或尋找 DUCX。
<!-- codex-mixin:zh-Hant:end -->

<!-- codex-mixin:en:start -->
## v0.3.11

### Emergency DUCC installation fix

- Fixed DUCC `.tar.zst` validation failures when the macOS GUI environment could not find Homebrew `zstd`. Terminal validation and App extraction now share one PATH and fall back to bzip2 when zstd is unavailable.
- DUCC installation failures now retain the stage, command, exit status, and original stderr.
- Removed the DUCX authentication option, managed installation paths, CLI flags, runtime bridge, and probes. Baidu OneAPI now offers only disabled or DUCC loopback.
- Legacy DUCX bridge values safely downgrade to disabled after upgrading and no longer launch or discover DUCX.
<!-- codex-mixin:en:end -->
