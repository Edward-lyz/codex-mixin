# 官方模型清单

官方模型元数据来自 [OpenAI Codex 仓库的 models.json](https://github.com/openai/codex/blob/main/codex-rs/models-manager/models.json)。实际下载地址是：

```text
https://raw.githubusercontent.com/openai/codex/main/codex-rs/models-manager/models.json
```

下载不需要 Codex CLI、CLI 版本或 OAuth 凭据，也不读取 Codex 的 `models_cache.json` 作为模型来源。下载模型清单本身不调用付费模型。
使用官方 GPT 时仍需要有效的官方账号；仓库清单不是账号权限、地区可用性或配额查询。老版本 Codex 客户端也可能不支持清单中的新能力。

## 更新与选择

在「模型与服务」选择官方 Provider 并刷新模型，或执行：

```bash
codex-mixin --no-tui provider discover official
codex-mixin --no-tui provider list --json
```

本机缓存为 `~/.codex-mixin/official-models.json`；自定义 `CODEX_GATEWAY_CONFIG` 时，缓存与配置文件同目录。缓存的 `codex_mixin_source` 字段记录固定的仓库来源。

刷新保留完整的官方元数据，包括 context window、reasoning levels、input modalities 和 hidden 条目。模型选择器不显示 `visibility=hide` 的条目。用户的模型选择与上下文覆盖继续由 Mixin 管理；新增模型可以在选择器中查看和选择，仓库出现的模型不代表账号一定能调用。

## 刷新频率与流量

网关启动后，官方托管目录和启用的动态 Provider 继续每 30 秒检查一次；本地能力目录每 15 秒同步一次，本地同步不下载官方清单。官方后台检查仅在启用官方 OAuth 托管目录时执行，禁用的自定义 Provider 不发后台模型发现请求。

官方文件和兼容 `/models` 的 GET 优先发送 `If-None-Match`，没有 ETag 时使用 `If-Modified-Since`。服务端返回 `304 Not Modified` 时复用已验证的响应体；返回 200 时校验并替换清单。没有匹配缓存却收到 304 会明确失败。手动刷新也使用条件请求，不强制重复下载。

官方校验信息保存在 `official-models.json` 的 `codex_mixin_http` 字段，重启后继续有效。自定义 Provider 的原始列表与校验信息只保存在进程内，按实际 URL 和认证请求头匹配；更换地址、API Key 或环境变量凭据后重新获取。自定义列表缓存最多 16 个条目，响应体合计最多 8 MiB，进程重启后首次请求重新下载。不保存认证请求头到磁盘，也不输出到日志。

长期运行的网关复用发现 HTTP 客户端，允许连接池复用。304 省去模型响应体，仍有请求头、响应头、TLS/连接维护和服务端校验开销，每条 30 秒轮询路径每天最多约 2,880 次检查（重启和手动刷新另计）。不支持校验头的服务端仍返回完整列表，因此不能保证所有 Provider 都节省流量。Baidu 等 POST 发现接口保持原语义；推理、付费探测和 SSE 不使用此缓存。

日志事件：

| 事件 | 含义 |
| --- | --- |
| `official_catalog_downloaded` | 官方清单响应体已下载并验证 |
| `official_catalog_not_modified` | 官方返回 304，复用本地清单 |
| `provider_models_downloaded` | 兼容 `/models` 响应体已下载并解析 |
| `provider_models_not_modified` | Provider 返回 304，复用进程内列表 |

模型列表之外的配额发现、models.dev 元数据和能力探测有各自的请求路径；304 模型列表并不代表整轮刷新完全没有其他网络请求。

## 失败与缓存

HTTP 下载、JSON、非空模型数组和唯一非空 slug 都通过校验后才原子写入缓存。错误内容不会替换已有缓存。

显式刷新失败会返回具体错误，保留旧缓存。生成或安装托管目录时，如果仓库暂时不可达且已有合法的 Mixin 缓存，会记录 `official_catalog_stale` 告警并继续使用旧缓存；首次没有缓存时明确失败，不再要求先运行 Codex CLI，也不偷偷切换到 Codex CLI 的缓存。

下载超时覆盖响应体，最大清单大小为 8 MiB。仓库的 `main` 会更新，因此不同日期的模型数量可能不同。

## 与 ECH 的关系

模型清单下载访问 `raw.githubusercontent.com`，使用正常网络路径，不属于官方 GPT 的 ECH 接管范围。模型调用仍访问官方 API，并按 [ECH 访问设置](ech-gpt.md) 选择连接方式。

官方清单下载成功和官方 GPT 请求成功是两件事。遇到 401、403、429 或模型不可用时，分别检查账号、权限、地区、配额和实际请求日志。
