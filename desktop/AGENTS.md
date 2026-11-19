# 桌面壳规范（GPUI）

继承根规范。这里是 macOS 菜单栏与 Windows 托盘共用的 GPUI 桌面壳，只消费真实 CLI contract，不实现网关、provider 或客户端业务。

- 独立 Cargo 工程（`desktop/Cargo.toml` 自带 `[workspace]`），GPUI 依赖不进入 CLI 的 workspace、lockfile 与门禁。应用只依赖 `gpui-kit`，GPUI、组件、图标都从它引入。
- `cli.rs` 以参数数组运行 `codex-mixin --no-tui --json-errors`，密钥只经子进程环境与 `@env:NAME` 传递；`model.rs` 是 CLI JSON 的类型视图。不读写 CLI 状态目录，路径从 `interface --json` 获取。
- `store.rs` 只放共享的轻量状态（网关状态、provider 快照、接口契约、额度）和串行的更改队列；页面草稿与页面数据由各页面实体持有。CLI 子进程在普通线程上运行，避免继承执行器线程的后台 QoS。
- 视觉：颜色、圆角来自 `themes/codex-mixin.json`，跟随系统浅深色；动效统一用 `theme.rs` 的三条三次贝塞尔曲线（移动、进入、退出）。新增界面不写裸色值。
- 文案放 `locales/app.yml`，每个 key 同时提供 `en` 与 `zh-CN`；运行 `./scripts/check_localizations.sh`。
- 构建：`cargo build --manifest-path desktop/Cargo.toml`；macOS 打包 `./desktop/macos/build_app.sh`，Windows 打包 `desktop/windows/package.ps1`。界面行为通过真实窗口人工验收，不新增 UI 自动化。
