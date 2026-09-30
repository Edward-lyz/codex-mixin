# ECH 代理访问 GPT（实验）

macOS 和 Windows 的「高级 → 启用 ECH 代理访问 GPT」使用
`https://edge.1molchuan.top/plus` 查询官方域名的 DNS HTTPS 记录和 IPv4
地址，再通过 rustls ECH 连接返回的地址。此功能默认关闭。

开关覆盖本地网关承载的官方 Responses HTTP/SSE、Responses WebSocket、
模型目录、压缩、图片、Realtime/Live 请求，以及指向 `chatgpt.com` 或
`api.openai.com` 的 OpenAI provider 调用和模型发现/能力检查。
第三方网关、本机服务、浏览器登录和其他进程的请求不受该开关控制。

## 连接与失败行为

- URL、HTTP 目标主机和证书验证保持原始官方域名。中继不需要解密 TLS。
- ECH 模式直接连接 DoH 返回的 IPv4，不读取现有代理环境变量；不使用
  IPv6，以免绕过帖子中描述的 IPv4 中继。不启用 HTTP/3。
- 仅在服务器接受 ECH 后发送账号凭据或模型请求。ECH 配置按 DNS TTL
  缓存；缓存过期重新查询，连接池随配置刷新。
- 启用前先对两个官方域名进行无凭据 TLS 和模型目录 GET 检查。测试失败
  会自动关闭功能、保存失败原因并切换官方流量到直连。
- 运行中 DNS/ECH/TLS 建连失败也会自动关闭。macOS 显示回退提示，Windows
  服务状态显示回退状态；高级菜单可以查看保存的原因。
- 自动回退只发生在连接阶段。已发送请求的超时或流中断不自动重发，
  HTTP 403/429 也不被当成 ECH 失败。不可复制的流式请求若在后续新连接
  上失败，会关闭 ECH 并返回明确错误；下一次请求走直连。
- 手动关闭会清除回退原因，并恢复原来的连接设置，包括原来的代理环境
  变量。保存设置成功、服务重启失败时，会明确报告两者的状态。

## CLI

```bash
codex-mixin --no-tui ech status --json
codex-mixin --no-tui ech test --json
codex-mixin --no-tui ech enable
codex-mixin --no-tui ech disable
```

`test` 不改变设置、不读取账号凭据，也不调用付费模型。返回每个域名的
解析地址、实际连接地址、ECH 接受状态、HTTP 状态、耗时和错误。
无凭据模型目录返回 401 表示接口可达。403/421/429 等响应不会误报为
功能可用；即使 TLS 接受了 ECH，启用检查也会失败并回退。

`enable` 和 `disable` 保存设置并重启 managed gateway。`status` 读取保存
的配置；若此前重启失败，保存的设置可能尚未应用到运行中的进程。

## 可验证范围

ECH 接受表示客户端与服务器完成了 ECH 握手；不能证明返回的地址是中继，
也不能证明最终出口位于 Azure 或具有某种 IP 信誉。
诊断中的 `relay_exit_verified` 始终为 `false`，直至有独立的出口验证能力。

2026-09-30 的现场检查确认两个官方域名接受 ECH，但 `/plus` 返回的地址
仍属于 Cloudflare。功能能使用该服务返回的地址；服务是否实际提供帖子
描述的中继，是服务端部署行为，客户端不能自行创造中继出口。

DNS 回归测试使用当天捕获的无凭据 DNS 响应，放在 `tests/fixtures/ech/`。
这些记录中的 ECH 公钥仅用于离线解析测试，不在生产路径中作为固定公钥。


现场对照实验的系统解析返回了 `198.18.0.0/15` 中的 Fake-IP 地址。因此，
这些结果不能证明关闭本机透明代理或 TUN 后仍可访问。`no_proxy` 只绕过
进程的显式代理设置，操作系统路由和透明代理仍会影响连接。
