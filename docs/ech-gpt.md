# ECH 代理访问 GPT（实验）

macOS 和 Windows 的「高级 → 启用 ECH 代理访问 GPT」使用
`https://edge.1molchuan.top/plus` 查询官方域名的 DNS HTTPS 记录和 IPv4
地址，再通过 rustls ECH 连接返回的地址。此功能默认关闭。

开关覆盖本地网关承载的官方 Responses HTTP/SSE、Responses WebSocket、
模型目录、压缩、图片、Realtime/Live 请求，以及指向 `chatgpt.com` 或
`api.openai.com` 的 OpenAI provider 调用和模型发现/能力检查。
第三方网关、本机服务、浏览器登录和其他进程的请求不受该开关控制。
Mixin 自身的[官方模型清单](official-model-catalog.md)现从 GitHub 公共文件下载，
这条 GitHub 请求使用正常网络路径，不属于 GPT 域名的 ECH 接管。

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

## 成功日志与实际请求证据

包含传输日志的构建在网关默认 `info` 级别记录以下事件。旧构建仅有 ECH
失败日志，需要更新网关并重启后才能看到成功事件。

| 事件 | 能证明什么 |
| --- | --- |
| `ech_tls_accepted` | 指定 socket 的服务器接受 ECH；包含 `host`、`peer_ip`、`local_ip` 和 `connection_kind` |
| `ech_client_ready` | DoH 查询和预检查已通过，ECH client/连接池已创建；包含 `transport_id` 和 DNS TTL |
| `official_http_request` | 请求开始发送，记录其固定的 `transport`、`request_id`、`request_kind` 和 `transport_id` |
| `official_http_response` | 已收到实际 HTTP 响应头；`transport=ech`、`ech_accepted=true` 表示该请求的 ECH 被接受，`peer_ip` 是该响应连接的实际 socket peer |
| `official_http_error` | 请求失败；记录连接错误/超时类别，不记录完整 URL |
| `official_websocket_connected` | WebSocket upgrade 成功；ECH 状态直接取自该 socket 的 rustls session，包含实际 peer 和 local 地址 |

`connection_kind=preflight` 是单独的无凭据预检查连接，不能单独证明业务
请求已经走 ECH。HTTP 日志中 `request_kind=gateway` 是网关请求，
`request_kind=diagnostic` 是 `ech test` 的无凭据检查。

`request_id` 是 Mixin 生成的本地关联 ID，不是转发给上游的 header。
同一请求回退重试保留 ID，并记录 `attempt=2`、`transport=direct_fallback`。
传输模式在创建请求时固定；并发请求关闭 ECH 后，不会把已准备的 ECH 请求
误标为直连。日志不记录凭据、正文、完整 URL、路径或 query。

`transport_id` 标识同一份 ECH 配置和 HTTP client/连接池；重复出现只证明
复用了 client，不能据此判断是否复用了同一条 TCP 连接。`phase=response_headers`
只表示收到响应头，不表示整段 SSE 或模型生成已经完成。

在 App 的「打开运行日志」查看网关日志，或使用：

```bash
codex-mixin --no-tui service logs --follow
# 只查看 ECH 和官方请求事件；也可直接过滤 App 显示的 gateway.log 路径。
rg 'ech_tls_accepted|ech_client_ready|official_http_|official_websocket_connected|official ECH disabled' ~/.codex-mixin/gateway.log
```

单独查看无凭据检查的成功日志：

```bash
RUST_LOG=codex_mixin=info codex-mixin --no-tui ech test --json
```

`transport=default` 表示正常客户端路径，可能使用既有显式代理；
`transport=direct_fallback` 表示关闭 ECH 后不使用进程的显式代理；
`transport=untracked` 表示缺少内部传输标记，不能据此判断 ECH 是否生效。

## 开着 TUN 会怎样

TUN 接管的是操作系统路由，ECH 加密的是 TLS 握手，两者可以同时生效。
普通 TCP/TUN 转发不需要解密 ECH。`no_proxy` 不会绕过 TUN，自动回退中的
「直连」也只表示不用进程级 HTTP/HTTPS 代理，仍可能经过 TUN。

mixin 在应用内使用 DoH，TUN 不一定看到原始 DNS 查询；ECH 又隐藏真实 SNI。
依赖系统 DNS 或 TLS SNI 的域名分流可能匹配不到 `chatgpt.com`，改由进程、
目标 IP、ECH 公共外层名字或默认规则决定路由。不要仅凭原有域名规则推断出口。
TLS 解密或拦截则可能使 ECH/证书验证失败，并触发自动关闭与回退。

`ech_accepted=true` 能证明握手成功，但不能证明未经过 TUN、使用了 `/plus`
中继或网站看到 Azure 出口。验证无 TUN 的效果需要在可恢复的独立测试中对照
开关状态；mixin 不会自动关闭用户的 TUN。
