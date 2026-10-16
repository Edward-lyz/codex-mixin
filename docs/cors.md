# 浏览器直连网关与 CORS

## 默认行为

默认允许**所有来源**，不需要预先配置白名单。`cors_allowed_origins` 的默认值是 `["*"]`；旧配置缺少该字段时也使用此默认值。网关对带 Origin 的请求回显其来源，包括浏览器沙箱或本地页面的 `Origin: null`。不启用 `Access-Control-Allow-Credentials`，不支持跨源 Cookie 凭据。

CORS 对所有网关路由生效，包括 OpenAI、Anthropic、Gemini API、404/405、鉴权和解析错误，以及 SSE。中间件只修改响应头，不读取、收集或重建响应体，保留 SSE 增量输出和背压。

**安全影响：默认模式允许任何网站通过浏览器调用本机网关。** CORS 不是身份认证；需要访问控制时，应启用 `client-auth` 并配置客户端凭据，或主动设置下述精确来源白名单。无 Origin 的 CLI/原生请求保持原来的行为。浏览器自身的 mixed-content、安全上下文、Local Network Access 权限和企业策略不能靠服务端 CORS 头绕过。

## 配置

以下示例使用已安装的 `codex-mixin`；开发时可替换为 `cargo run --locked --`。

读取当前策略，不会仅因读取默认值而创建配置：

```sh
codex-mixin --no-tui config cors get --json
```

恢复允许所有来源（引号防止 shell 展开星号）：

```sh
codex-mixin --no-tui config cors set --origin '*' --json
```

可选：限制为精确来源，可重复传入 `--origin`，替换整个列表：

```sh
codex-mixin --no-tui config cors set \
  --origin 'https://your-web-app.example' \
  --origin 'http://localhost:8723' --json
```

可选：禁用跨源浏览器调用：

```sh
codex-mixin --no-tui config cors set --disable --json
```

除单独的 `*` 外，列表项必须为浏览器序列化的 HTTP(S) Origin：`scheme://host[:port]`，例如 `https://example.com`。路径、用户信息、查询、fragment、末尾 `/`、部分通配域名、显式默认端口和 `null` 列表项被拒绝。`null` 请求来源在全来源模式下已被允许，无需单独配置。显式空数组 `[]` 表示禁用跨源调用。此前已保存的显式白名单不会被更新二进制自动覆盖；需要时执行恢复全来源命令。

修改沿用配置锁、加密和原子保存，不覆盖其他配置字段。无效输入非零退出且不写入；`--json-errors` 返回既有 JSON 错误 envelope。成功 JSON 包含 `cors_allowed_origins` 和 `restart_required`。**修改后需重启正在运行的网关**：

```sh
codex-mixin --no-tui restart
```

## HTTP 契约

获准来源的普通、错误和 SSE 响应包含：

```text
Access-Control-Allow-Origin: <请求来源>
Access-Control-Allow-Methods: GET, POST, OPTIONS
Access-Control-Allow-Headers: authorization, x-api-key, anthropic-version, anthropic-beta, content-type, x-goog-api-key
Access-Control-Max-Age: 86400
Vary: Origin
```

OPTIONS 在鉴权、解压和业务之前返回 **204 空响应**，不存在的路由也一样，不要求 Authorization。预检回显全部 `Access-Control-Request-Headers`；明确请求 `Access-Control-Request-Private-Network: true` 时才返回 PNA 授权。普通响应不附带 PNA 授权。已有 Vary 不会被覆盖，预检另对请求方法、请求头和 PNA 设置 Vary。

跨源方法仍限 GET、POST、OPTIONS。重复 Origin 被拒绝。显式白名单模式下，未列出的跨源来源返回 403，不执行业务；严格本地同源例外同时核对实际 listener 端口、本地地址和 Host，不能通过任意 DNS/Host 绕过白名单。

## 本机验收与测试

将 `<PORT>` 替换为实际网关端口。在默认全来源模式中，可使用任意网页来源：

```sh
curl -i -X OPTIONS 'http://127.0.0.1:<PORT>/v1/models' \
  -H 'Origin: https://any-web-app.example' \
  -H 'Access-Control-Request-Method: POST' \
  -H 'Access-Control-Request-Headers: authorization,content-type' \
  -H 'Access-Control-Request-Private-Network: true'
```

预期 204，Allow-Origin 等于请求来源，Allow-Headers 回显请求头，Allow-Private-Network 为 true，且没有 Allow-Credentials。只有主动启用精确白名单后，未授权来源才应返回 403。

```sh
cargo test --locked --test cors --test cors_cli
cargo test --locked --features client-auth --test cors --test cors_cli
```

测试驱动真实 Router、配置存储和 CLI；SSE 使用受控本地 provider，验证 Messages、Chat、Responses 先收到首帧，再释放上游完成闸门并检查终止事件。它们不是浏览器实机或付费上游验证。
