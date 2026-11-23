# 桌面壳规范（GPUI）

继承根规范。这里是 macOS、Windows 和 Linux 共用的 GPUI 桌面壳，只消费真实 CLI contract，不实现网关、provider 或客户端业务。

- 独立 Cargo 工程（`desktop/Cargo.toml` 自带 `[workspace]`），GPUI 依赖不进入 CLI 的 workspace、lockfile 与门禁。应用只依赖 `gpui-kit`，GPUI、组件、图标都从它引入。
- `cli.rs` 以参数数组运行 `codex-mixin --no-tui --json-errors`，密钥只经子进程环境与 `@env:NAME` 传递；`model.rs` 是 CLI JSON 的类型视图。不读写 CLI 状态目录，路径从 `interface --json` 获取。
- `store.rs` 只放共享的轻量状态（网关状态、provider 快照、接口契约、额度）和串行的更改队列；页面草稿与页面数据由各页面实体持有。CLI 子进程在普通线程上运行，避免继承执行器线程的后台 QoS。
- 视觉：颜色、圆角来自 `themes/codex-mixin.json`，跟随系统浅深色；动效统一用 `theme.rs` 的三条三次贝塞尔曲线（移动、进入、退出）。新增界面不写裸色值。
- 文案放 `locales/app.yml`，每个 key 同时提供 `en` 与 `zh-CN`；运行 `./scripts/check_localizations.sh`。
- 构建：`cargo build --manifest-path desktop/Cargo.toml`；macOS 打包 `./desktop/macos/build_app.sh`，Windows 打包 `desktop/windows/package.ps1`，Linux 打包 `desktop/linux/package.sh`。界面行为通过真实窗口人工验收，不新增 UI 自动化。
- 窗口：控制台打开时 App 切到 `ActivationPolicy::Regular`（Dock、Command-Tab、菜单栏与 Command-W 等快捷键），全部关闭后回到 `Accessory`；菜单与快捷键在 `console::init`。macOS 关闭了 Touch Bar API（`NSFunctionBarAPIEnabled`），避免关闭窗口时 AppKit 的 KVO 竞态崩溃。
- 更新：`updater.rs` 从 GitHub Release 下载 `codex-mixin-app-<版本>-<target>`（macOS 和 Windows 用 `.zip`，Linux 用 `.tar.gz`），用内置 Ed25519 公钥校验 `.sig` 后由新版本的辅助进程应用。发布包由 `scripts/sign_update.sh` 用 `UPDATE_SIGNING_KEY` 签名。macOS 默认 ad-hoc 签名并固定 designated requirement，配置 Developer ID 证书与 notary 凭据后启用 hardened runtime 和公证。
