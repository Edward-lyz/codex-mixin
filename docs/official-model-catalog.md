# 官方模型清单

官方 GPT 模型的元数据直接取自 OpenAI Codex 仓库里的 [models.json](https://github.com/openai/codex/blob/main/codex-rs/models-manager/models.json)，实际下载地址是：

```text
https://raw.githubusercontent.com/openai/codex/main/codex-rs/models-manager/models.json
```

下载不需要安装 Codex CLI，也不需要 OAuth 凭据，不会调用付费模型；Codex 本地的 `models_cache.json` 也不再作为来源。

这份清单只说明仓库里有哪些模型。能不能调用，仍取决于官方账号的权限、地区和配额；旧版 Codex 客户端也可能不支持清单里的新能力。

## 刷新和选择

在「模型与服务」里选中官方 Provider 后刷新模型，或者运行：

```bash
codex-mixin --no-tui provider discover official
codex-mixin --no-tui provider list --json
```

缓存文件是 `~/.codex-mixin/official-models.json`；设置了 `CODEX_GATEWAY_CONFIG` 时，缓存和配置文件放在同一目录。文件里的 `codex_mixin_source` 字段记录清单来源。

刷新会保留完整的元数据，包括 context window、reasoning levels、input modalities 和隐藏条目，其中 `visibility=hide` 的模型不会出现在选择器里。选了哪些模型、上下文怎么覆盖，仍由 Mixin 管理。

## 后台刷新和流量

网关运行时，每 30 秒检查一次官方托管目录和已启用的动态 Provider；本地能力目录每 15 秒同步一次，这一步不下载官方清单。只有启用了官方 OAuth 托管目录才会检查官方清单，已禁用的自定义 Provider 不会发模型发现请求。

这些 GET 都是条件请求：有 ETag 时带 `If-None-Match`，没有时带 `If-Modified-Since`。服务端返回 `304 Not Modified`，就复用上次校验过的内容；返回 200，就校验后替换。本地没有对应缓存却收到 304，会直接报错。手动刷新同样走条件请求。

两类缓存的保存方式不同。官方清单的校验信息写在 `official-models.json` 的 `codex_mixin_http` 字段里，重启后仍然有效。自定义 Provider 的列表和校验信息只保存在内存中，按实际 URL 和认证头区分；改了地址、API Key 或凭据环境变量，就会重新获取。内存缓存最多 16 条、合计 8 MiB，重启后第一次请求会重新下载。认证头不写盘，也不进日志。

304 能省掉响应体，但请求头、响应头、TLS 和连接维护的开销还在。每条 30 秒的轮询一天最多约 2,880 次（重启和手动刷新另算）。不支持校验头的服务端每次仍会返回完整列表。Baidu 等用 POST 做模型发现的接口保持原来的方式；推理、付费探测和 SSE 都不用这套缓存。

相关日志事件：

| 事件 | 含义 |
| --- | --- |
| `official_catalog_downloaded` | 下载并校验了新的官方清单 |
| `official_catalog_not_modified` | 官方返回 304，继续用本地清单 |
| `provider_models_downloaded` | 下载并解析了兼容 `/models` 接口的新列表 |
| `provider_models_not_modified` | Provider 返回 304，继续用内存中的列表 |

配额查询、models.dev 元数据和能力探测各自有请求，所以模型列表返回 304，不代表这一轮刷新没有其他网络请求。

## 失败和缓存

新清单要依次通过 HTTP 下载、JSON 解析、模型数组非空、每个 slug 非空且唯一这几项检查，才会原子写入缓存。内容有问题时，旧缓存保持原样。

手动刷新失败会返回具体错误，旧缓存保留。生成或安装托管目录时，如果 GitHub 暂时访问不了而本地有合法缓存，会记录 `official_catalog_stale` 告警并继续用旧缓存；如果本地从来没有缓存，就直接报错，也不会转去读 Codex CLI 的缓存。

下载超时包含读取响应体的时间，清单大小上限是 8 MiB。仓库的 `main` 分支会更新，不同日期看到的模型数量可能不同。

## 和 ECH 的关系

清单从 `raw.githubusercontent.com` 下载，走普通网络，不受 ECH 开关影响。模型调用仍然发往官方 API，按 [ECH 设置](ech-gpt.md) 决定连接方式。

清单下载成功，不代表官方 GPT 请求一定成功。遇到 401、403、429 或模型不可用时，分别检查账号、权限、地区、配额和实际请求日志。
