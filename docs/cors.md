# 浏览器直连网关与 CORS

## 范围与安全边界

CORS 由 Rust CLI/core 管理，对所有网关路由生效，包括 OpenAI、Anthropic、Gemini API、404/405、鉴权和解析错误，以及 SSE 响应。它不依赖 GUI，也不修改模型转发规则。

默认白名单只有附件申请的来源 `http://10.252.145.171:8723`。只回显精确匹配的来源，不返回 `Access-Control-Allow-Origin: *`，不启用 Cookie credentials。已有配置缺少 `cors_allowed_origins` 时沿用这个默认值；显式空数组表示禁用跨源调用。

具有 `Origin` 但不在白名单的跨源请求返回 **403**，且不会进入鉴权或业务处理。`null`、重复 Origin、路径、用户信息、通配符以及混合来源均不被允许。没有 `Origin` 的 CLI/原生客户端请求保持原来的行为。严格本地同源请求仍可使用：Origin 与 HTTP Host 一致、端口等于实际 listener 端口，且 host 为 `localhost`、loopback IP 或显式绑定 IP；任意 DNS 域名不能通过伪造一致的 Host 绕过白名单。

CORS 不是身份认证。没有 Origin 的非浏览器客户端不受 CORS 限制；需要认证时仍需启用项目的 `client-auth` 构建能力并配置对应凭据。浏览器自身的 mixed-content、安全上下文、Local Network Access 权限或企业策略仍可能阻止请求，这些限制不能靠服务端响应头绕过。

## CLI 配置

以下命令从仓库根运行；已安装时可用 `codex-mixin` 替代 `cargo run --locked --`。不需要桌面环境或可用的上游模型。

读取有效的已保存白名单；没有配置时只展示默认值，不创建配置：

```sh
cargo run --locked -- --no-tui config cors get --json
```

替换整个白名单，可重复传入 `--origin`：

```sh
cargo run --locked -- --no-tui --json-errors config cors set \
  --origin 'https://your-web-app.example' \
  --origin 'http://localhost:8723' --json
```

禁用所有跨源浏览器调用：

```sh
cargo run --locked -- --no-tui config cors set --disable --json
```

来源必须为浏览器序列化的 HTTP(S) Origin：`scheme://host[:port]`，例如 `https://example.com`。不要加末尾 `/`、路径、查询、fragment、用户信息或显式默认端口（如 `http://example.com:80` 应写为 `http://example.com`）；域名需小写，IPv6 地址用方括号。`*` 与 `null` 配置被拒绝。

修改通过现有配置锁、加密和原子替换保存，不覆盖其他配置字段。校验失败时进程非零退出，`--json-errors` 保留 stderr 最后一行的既有错误 envelope，主配置不变。成功 JSON 包含 `cors_allowed_origins` 和 `restart_required`。**修改后需重启正在运行的网关**；配置命令不会自动重启或刷新客户端 catalog：

```sh
cargo run --locked -- --no-tui restart
```

首次使用需先按现有 provider 配置流程完成上游设置。也可在导入的配置中设置同名 `cors_allowed_origins` 数组；加载、导入、保存和创建网关状态均校验来源。

## HTTP 契约

对白名单来源的普通、错误和 SSE 响应添加：

```text
Access-Control-Allow-Origin: <精确请求来源>
Access-Control-Allow-Methods: GET, POST, OPTIONS
Access-Control-Allow-Headers: authorization, x-api-key, anthropic-version, anthropic-beta, content-type, x-goog-api-key
Access-Control-Max-Age: 86400
Vary: Origin
```

跨源只允许 GET、POST、OPTIONS。OPTIONS 位于鉴权、解压与业务逻辑之前，对不存在的路由也返回 **204 空响应**；不要求 Authorization。预检的 `Access-Control-Request-Headers` 存在时回显全部请求值，因此额外客户端请求头不必逐个配置。

只有白名单来源的 OPTIONS 请求明确携带 `Access-Control-Request-Private-Network: true` 时，才返回 `Access-Control-Allow-Private-Network: true`。普通响应和未获准来源不获得该授权。

预检还对 `Access-Control-Request-Method`、`Access-Control-Request-Headers`、`Access-Control-Request-Private-Network` 设置 Vary；已有压缩/上游的 Vary 不会被覆盖。中间件只修改响应头，不读取、收集或重建响应体，SSE 保持增量输出和背压。

## 本机验收

前提：启动网关并从 CLI 状态获取实际端口，将 `<PORT>` 替换为该端口，`<WEB_ORIGIN>` 替换为白名单来源。

```sh
curl -i -X OPTIONS 'http://127.0.0.1:<PORT>/v1/models' \
  -H 'Origin: <WEB_ORIGIN>' \
  -H 'Access-Control-Request-Method: POST' \
  -H 'Access-Control-Request-Headers: authorization,content-type,x-api-key,anthropic-version,anthropic-beta,x-goog-api-key' \
  -H 'Access-Control-Request-Private-Network: true'
```

预期状态 204，Allow-Origin 等于请求来源，Allow-Headers 包含所请求头，Allow-Private-Network 为 true。换成未授权来源应返回 403，且没有 Allow-Origin/PNA 授权。用真实浏览器验证时还应检查其控制台的 mixed-content 或本地网络权限提示。

## 定向回归

```sh
cargo test --locked --test cors --test cors_cli
cargo test --locked --features client-auth --test cors --test cors_cli
cargo test --locked --lib config::
cargo test --locked --lib server::
cargo test --locked --test gateway_http --test websocket_proxy
```

CORS 测试驱动真实 Router、配置存储和 CLI 二进制。SSE 用受控本地 provider 验证 Messages、Chat Completions 和 Responses 三条链路：先收到响应头与第一帧，再释放上游完成闸门，最后确认终止事件，避免仅凭 HTTP 200 判断流式可用。
