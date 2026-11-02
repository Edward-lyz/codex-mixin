# GitHub CI 耗时与缓存

2026-11-02：工作流配置已调整，本地静态验证通过；尚未在 GitHub 上运行新配置。以下收益为推算，不是实测提速结果。

## 参考文章与适用范围

Owen Gretzinger 的 [How I made our CI 20x faster (steal this)](https://x.com/owengretzinger/status/2116291924219060319) 提供了九项优化。X 页面直接访问返回 403，已通过 [FxTwitter API](https://api.fxtwitter.com/status/2116291924219060319) 读取其 article 正文，核对标题与内容。

本项目采用其中的耗时分解、修复缓存和缩短串行关键路径。其余方法按当前工具链和证据取舍：

| 方法 | 本项目处理 |
| --- | --- |
| 更换 runner、购买更多 CPU | 本次样本启动只需数秒；暂不引入付费服务 |
| 更换 JS linter/typechecker | 主要耗时在 Rust 编译，继续使用现有严格 Clippy |
| 更多分片、共享测试 worker | Windows 已有三组分片；保留真实 CLI 测试的状态隔离 |
| 修复缓存和 setup | 按 Clippy、integration、client-auth、release 分开缓存 |
| 并行执行 | Unix 的默认测试、client-auth、release 构建独立调度 |
| 按变更跳过测试、pre-push hook | 本次不引入；保持全部门禁与现有本地开发流程 |

## 改前基线

从 GitHub 的 job/step 时间戳与日志取样，包含启动和收尾开销，不代表历史分位数：

| main 提交 / workflow | 总耗时 | 主要耗时 |
| --- | --- | --- |
| `355c520` / [CI](https://github.com/Edward-lyz/codex-mixin/actions/runs/41379800355) | 9m04s | macOS Clippy 44s、集成测试 2m35s，之后才构建 App 4m53s；Linux musl 构建 3m20s 排在默认测试及 auth 之后 |
| `355c520` / [Windows](https://github.com/Edward-lyz/codex-mixin/actions/runs/41379800327) | 14m28s | release CLI 编译 9m44s，其后还有 CLI contract、打包和安装升级验证 |
| `c687c76` / [CI](https://github.com/Edward-lyz/codex-mixin/actions/runs/41275157935) | 5m43s | Linux musl 构建 2m29s；macOS App 构建 2m59s，均与测试串行 |
| `c687c76` / [Windows](https://github.com/Edward-lyz/codex-mixin/actions/runs/41275158059) | 14m25s | release CLI 编译 9m29s |

最新 Windows run 的五个 Rust job 都恢复同一个约 415 MiB 的缓存：`v0-rust-rust-Windows_NT-x64-e70cdc14-a5d28a27`。该 main 缓存于 2026-11-01 04:06:05 UTC 创建；对应 [run](https://github.com/Edward-lyz/codex-mixin/actions/runs/41271612250) 中 Clippy 最早结束。2026-11-02 的 release 日志仍从依赖开始编译：完整构建为 02:50:43–03:00:27 UTC，直到 02:54:30 UTC 才开始编译项目本身，其中项目编译和链接仍约需 5m57s。

[GitHub 缓存不可修改](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)，而 [rust-cache 的 shared-key 会替换 job 标识](https://github.com/Swatinem/rust-cache#example-usage)。因此共用 `shared-key: rust` 会让不同编译类型竞争第一份缓存，后完成的 release 无法补充依赖。本项目已有 main 缓存，PR 可读 base branch 缓存；没有证据支持将文章中的 PR 缓存缺失直接当作本项目根因。

## 当前配置

- [CI](../.github/workflows/ci.yml) 保留原有两个 `Test` job 的名称、Clippy、完整集成测试、Linux 格式/语法/架构检查和 multi-provider E2E；使用 `default-tests` 缓存键。
- 独立 `Client auth` job 执行原有 feature Clippy、定向测试以及之后的 prompt-cache E2E。feature 编译产物与默认测试分开。
- 独立 Unix `Build` 矩阵并行构建原有静态 Linux CLI 与 macOS App，继续验证静态链接和图标。macOS 显式使用 `aarch64-apple-darwin`，与打包工作流的 target 布局一致。
- [Windows](../.github/workflows/windows.yml) 使用 `windows-clippy`、`windows-integration` 和 `release-x86_64-pc-windows-msvc` 三种 shared-key；integration 三组使用相同 features/profile，由 `integration-b` 保存依赖缓存，其他两组只恢复。CLI contract、WinUI 构建、打包及安装升级验证保持原样。
- [Build](../.github/workflows/build.yml) 与 CI 按 `release-<target>` 共享 release 依赖缓存。工具链、runner OS/architecture、Cargo 配置及锁文件仍参与 rust-cache 的自动哈希；不同 feature 或编译环境不能仅凭相同 shared-key 假定可复用。

没有关闭测试、修改 release 的 `lto = "thin"` / `codegen-units = 1`，也没有新增自定义构建函数或发布动作。仓库检查时没有 ruleset，main 返回未设置 branch protection；若后续启用保护，应将新增 `Client auth` 与两个 `Build` check 一并设为必选。

## 验证与后续测量

本次仅改 YAML 与本文档。本地执行 `actionlint 1.7.12` 检查全部四份工作流、`git diff --check`、`./scripts/check_arch.sh`，均通过；逐项核对原有门禁仍在对应平台运行。未重新编译无变化的 Rust/GUI，也未以本地静态检查替代 GitHub 的三平台运行。

按最新基线推算：Unix 将构建与测试分开后，关键路径约为 5–6 分钟；Windows 热缓存有望省去约 3–4 分钟依赖编译。项目 release 编译/链接及安装器验证仍需运行，不承诺 20 倍收益。首次新键会冷启动，拆 job 也会增加 runner 设置和恢复缓存的总开销。

上线后，至少比较一次新键冷运行和一次相同 Cargo/toolchain 配置的 main 热运行，并检查一个 PR 能恢复 main 缓存。从仓库根执行：

```sh
gh run list --repo Edward-lyz/codex-mixin --branch main --limit 10
gh run view <run-id> --repo Edward-lyz/codex-mixin --json jobs
gh run view <run-id> --repo Edward-lyz/codex-mixin --log
gh cache list --repo Edward-lyz/codex-mixin --limit 100
```

分别记录排队/启动、checkout/toolchain/cache/setup、编译、测试、打包与 cache-save 时间；既看墙钟关键路径，也看总 job 时间与缓存占用。热运行应不再全量编译 release 第三方依赖，所有原有门禁应继续通过。旧缓存交由 GitHub 淘汰，不主动删除远端缓存。
