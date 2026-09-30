# ECH 代理访问 GPT（实验）

这是一个默认关闭的实验功能。打开后，Codex Mixin 连接 OpenAI 官方域名前，会先向 `https://edge.1molchuan.top/plus` 做 DoH 查询，拿到官方域名的 HTTPS 记录（含 ECH 配置）和 IPv4 地址，再用 rustls 以 ECH 方式连接这些地址。ECH 会加密 TLS 握手中的真实域名（SNI）。

入口在 macOS 和 Windows App 的「高级 → 启用 ECH 代理访问 GPT」，也可以用下文的 CLI。

## 哪些请求走 ECH

只有本地网关发往 `chatgpt.com` 和 `api.openai.com` 的请求：官方 Responses（HTTP/SSE 和 WebSocket）、模型目录、压缩、图片、Realtime/Live，以及指向这两个域名的 OpenAI Provider 的调用、模型发现和能力检查。

第三方网关、本机服务、浏览器登录和其他进程都不受这个开关影响。[官方模型清单](official-model-catalog.md)从 GitHub 下载，也走普通网络。

## 怎么连接

- URL、HTTP Host 和证书校验仍使用官方域名，中间节点不需要解开 TLS。
- ECH 模式直接连接 DoH 返回的 IPv4 地址，忽略代理环境变量。它不用 IPv6，避免绕开该服务提供的 IPv4 地址；也不用 HTTP/3。
- 服务器接受 ECH 之后，才会发送账号凭据和模型请求。
- ECH 配置按 DNS TTL 缓存，过期后重新查询，连接池随之刷新。

## 失败时怎么处理

启用前，Mixin 会对两个官方域名各做一次不带凭据的 TLS 握手和模型目录 GET。任何一项没通过，功能就保持关闭，失败原因会被保存，官方流量继续直连。

运行中如果 DNS、ECH 或 TLS 建连失败，功能同样会自动关闭。macOS 会弹出提示，Windows 在服务状态里显示已回退，两边都可以在高级菜单里查看保存的原因。

自动回退只发生在建立连接这一步。请求已经发出后再超时或断流，Mixin 不会自动重发；HTTP 403、429 也不算 ECH 失败。无法重放的流式请求如果在新连接上失败，Mixin 会关闭 ECH，把错误返回给客户端，下一次请求改走直连。

手动关闭会清除回退原因，并恢复原来的连接设置，包括代理环境变量。如果设置已保存但服务重启失败，界面会分别告诉你这两步的结果。

## CLI

```bash
codex-mixin --no-tui ech status --json
codex-mixin --no-tui ech test --json
codex-mixin --no-tui ech enable
codex-mixin --no-tui ech disable
```

`ech test` 只做检查：不改设置，不读账号凭据，也不调用付费模型。它会列出每个域名解析到的地址、实际连接的地址、ECH 是否被接受、HTTP 状态、耗时和错误。不带凭据请求模型目录得到 401，说明接口可达；得到 403、421、429 等则判为不可用。这时即使 TLS 已经接受了 ECH，`enable` 也会失败并回退。

`enable` 和 `disable` 保存设置后会重启托管网关。`status` 读的是保存下来的配置；如果上一次重启失败，运行中的网关可能还没用上新设置。

## ECH 被接受说明了什么

`ech_accepted=true` 只说明这次握手用了 ECH，而且服务器接受了。它不能说明连接的地址是中继，也不能说明出口在 Azure 或 IP 信誉如何。所以诊断输出里的 `relay_exit_verified` 一直是 `false`，等以后有独立的出口验证手段才会改变。

2026-09-30 的实测结果：两个官方域名都接受 ECH，但 `/plus` 返回的地址属于 Cloudflare。Mixin 只负责连接这个服务返回的地址，服务端有没有做中继，由服务方决定。

同一次实测里，系统 DNS 返回的是 `198.18.0.0/15` 段的 Fake-IP，说明测试机开着透明代理或 TUN。因此这组结果不能说明关掉 TUN 后仍然可以访问。

离线 DNS 回归测试用的是当天抓到的无凭据 DNS 响应，放在 `tests/fixtures/ech/`。其中的 ECH 公钥只用于解析测试，生产代码不会写死它。

## 用日志确认请求走了 ECH

新版网关在默认的 `info` 级别记录下面这些事件。旧版本只记录 ECH 失败，需要更新并重启网关后才能看到成功事件。

| 事件 | 含义 |
| --- | --- |
| `ech_tls_accepted` | 某个 socket 的 ECH 握手被服务器接受，带 `host`、`peer_ip`、`local_ip` 和 `connection_kind` |
| `ech_client_ready` | DoH 查询和启用前检查已通过，ECH client 和连接池已创建，带 `transport_id` 和 DNS TTL |
| `official_http_request` | 请求开始发送，记录 `transport`、`request_id`、`request_kind` 和 `transport_id` |
| `official_http_response` | 收到 HTTP 响应头。`transport=ech` 且 `ech_accepted=true` 表示这个请求用了 ECH，`peer_ip` 是这条连接实际的对端地址 |
| `official_http_error` | 请求失败，记录连接错误或超时类别 |
| `official_websocket_connected` | WebSocket 升级成功。ECH 状态直接取自这个 socket 的 rustls session，带实际的对端和本地地址 |

读日志时注意这几点：

- `connection_kind=preflight` 是启用前的无凭据检查，不能证明业务请求走了 ECH。要看 `request_kind=gateway` 的 `official_http_response`；`request_kind=diagnostic` 来自 `ech test`。
- `request_id` 是 Mixin 在本地生成的关联 ID，不会发给上游。回退重试沿用同一个 ID，并带上 `attempt=2` 和 `transport=direct_fallback`。
- 每个请求的传输方式在创建时就确定了。并发请求期间 ECH 被关闭，已经准备好的 ECH 请求仍然标为 ECH。
- `transport_id` 对应一份 ECH 配置及其 HTTP client 和连接池。同一个 ID 反复出现，只说明复用了 client，不说明复用了同一条 TCP 连接。
- `phase=response_headers` 只表示收到了响应头，SSE 流或模型生成可能还没结束。
- `transport` 还有三个取值：`default` 是普通路径，可能经过已配置的显式代理；`direct_fallback` 是关闭 ECH 后的直连，不走进程级代理；`untracked` 表示缺少内部标记，无法据此判断。
- 日志不记录凭据、请求正文、完整 URL、路径和 query。

在 App 里点「打开运行日志」，或者在终端运行：

```bash
codex-mixin --no-tui service logs --follow
# 只看 ECH 和官方请求事件，也可以直接过滤 App 显示的 gateway.log 路径
rg 'ech_tls_accepted|ech_client_ready|official_http_|official_websocket_connected|official ECH disabled' ~/.codex-mixin/gateway.log
```

只看无凭据检查的日志：

```bash
RUST_LOG=codex_mixin=info codex-mixin --no-tui ech test --json
```

## 和 TUN 一起用

TUN 接管的是系统路由，ECH 加密的是 TLS 握手，两者可以同时生效，普通的 TCP 转发也不需要解开 ECH。但有几件事会和你的预期不同：

- `no_proxy` 和自动回退里的“直连”，只表示不走进程级 HTTP/HTTPS 代理，流量仍可能经过 TUN。
- Mixin 在应用内做 DoH，TUN 未必看得到 DNS 查询；ECH 又隐藏了真实 SNI。依靠 DNS 或 SNI 按域名分流的规则可能匹配不到 `chatgpt.com`，流量最终按进程、目标 IP、ECH 外层域名或默认规则走。不要按原来的域名规则推断出口。
- 会解密或拦截 TLS 的代理可能导致 ECH 或证书校验失败，进而触发自动关闭和回退。

`ech_accepted=true` 同样不能证明流量没经过 TUN、用了 `/plus` 中继，或目标网站看到的是 Azure 出口。想验证不开 TUN 的效果，需要在可以随时恢复的环境里分别测试开关的两种状态。Mixin 不会替你关闭 TUN。
