# Web 控制面板壳规范

继承根规范。这里是 axum + 静态资源的 Web 交互壳，由 `src/main.rs` 组合，不成为 Rust 库或 CLI 私有模块的依赖。

- `api` 负责 HTTP 路由、鉴权与响应，`actions` 把白名单动作映射为 CLI 参数数组，`cli` 以 `--no-tui --json-errors` 运行真实 CLI 子进程；`assets/` 只做渲染与交互。不复制 provider、网关或配置业务规则，不直写 CLI 状态目录。
- 所有 `/api/*` 必须校验每次启动生成的 token（常量时间比较）；静态资源不含状态或密钥。默认监听 `0.0.0.0`，新增接口不得绕过鉴权。
- 用户输入在 `actions` 校验：以 `--flag=value` 传值，位置参数拒绝 `-` 开头与控制字符，拒绝 `@env:` 字面值；密钥只经子进程环境变量与 `@env:NAME` 传递，不进 argv、日志或响应。可执行路径、header env 等高风险参数不对 Web 开放。
- 变更动作串行执行，并发返回 409；子进程有超时，断连或超时即终止。错误沿用 CLI envelope 的 `committed` 与 `stage`，未知提交状态为 null。
- 前端为无构建步骤的 HTML/CSS/原生 JS，遵守 CSP（无内联脚本/样式属性、无外部网络资源）；配色每个主题不超过 4 种基础色，其余色阶只由透明度派生；额度圆环另有 `--ring-1..4` 四个数据可视化色，仅用于区分同心圆，不含告警红；动画尊重 `prefers-reduced-motion`；保持键盘可达、ARIA 标注与 AA 对比度。
- 运行 `cargo test --locked --test panel_interaction` 与根级门禁；界面改动需在真实浏览器中检查明暗主题与窄屏布局。
