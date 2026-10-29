# 官方模型清单

官方 GPT 模型的公共模板取自 OpenAI Codex 仓库里的 [models.json](https://github.com/openai/codex/blob/main/codex-rs/models-manager/models.json)，实际下载地址是：

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

模型发现缓存是 `~/.codex-mixin/official-models.json`，公共模板缓存是 `~/.codex-mixin/official-template.json`；设置了 `CODEX_GATEWAY_CONFIG` 时，缓存和配置文件放在同一目录。文件里的 `codex_mixin_source` 字段记录清单来源。

刷新会保留完整的元数据，包括 context window、reasoning levels、input modalities 和隐藏条目，其中 `visibility=hide` 的模型不会出现在选择器里。选了哪些模型、上下文怎么覆盖，仍由 Mixin 管理。

已登录官方订阅账号时，Codex 托管目录以当前账号模型发现缓存确定模型集合和可见性。
公共模板仍有、但当前账号已不再返回的模型不会进入托管目录；账号新增而模板尚未收录的
模型使用账号返回的元数据。同名官方模型的
`context_window`、`max_context_window` 和 `auto_compact_token_limit` 与官方账号返回值同步，
同时保留公共模板的指令和工具定义。这一步只合并本地元数据，不增加网络请求、模型探测
或 token 消耗；安装、手动同步和后台刷新使用相同规则。自定义 Provider 模型不受影响。
账号切换或退出后不再使用旧账号缓存。已登录但没有匹配的账号目录时，同步报错，需先刷新
OpenAI 模型列表；不会用公共模板代替账号的模型集合。未登录时仍可用公共模板准备接入。
账号未提供的窗口字段保留模板值，显式 `null` 则清除原值；非法数值或窗口超过最大窗口时
报错，不写入生成的托管目录。

## 模型顺序

默认按模型家族分组、版本号从新到旧排列。`Claude`、`Opus`、`Sonnet`、`Haiku` 归为
同一家族，同版本按 Opus、Sonnet、Haiku 排列；`auto` 放在列表末尾。已有配置未指定排序
模式时也使用此默认顺序。

Web 面板在「模型与服务」中拖动已选模型，或使用上移、下移按钮（键盘 Alt + ↑/↓），然后点击
「保存模型选择」。macOS 原生 App 在模型列表的「顺序」列提供拖动及上移、下移按钮；
通过「模型顺序」选择「家族与版本」或「手动排序」，再点击「应用更改」。拖动或移动模型会
进入手动排序；恢复家族与版本后由 CLI 在应用时计算默认顺序。

保存后，模型顺序会同步到 Codex 目录的 `priority`；后续模型刷新保留
手动顺序，新发现并自动加入的模型追加到末尾。Web 面板的更多菜单也可恢复家族与版本默认顺序，
同样需要保存。两种界面的表头测速、上下文等排序只用于查看，不改变保存的模型顺序。
macOS 仅修改上下文时不会重写所选模型或排序模式；后台刷新保留未应用的排序草稿，
外部更改会沿用已有冲突提示和重新载入/放弃草稿流程。至少保留一个已选模型后才能应用。

CLI 使用相同能力，可在仓库根或任意已安装 CLI 的目录运行：

```bash
codex-mixin --no-tui providers select <provider-id> --model-order manual --model <first-model> --model <second-model>
codex-mixin --no-tui providers select <provider-id> --model-order family
```

`<provider-id>` 也可以是 `official`。只传 `--model-order` 时保留已选模型集合；显式传入
模型列表且省略排序模式时，按传入顺序保存为手动模式。单独修改上下文不会改变排序模式。

官方模型的 `official_model_contexts` 覆盖同时用于 Codex 和其他托管客户端的
模型配置，包括 Pi、OpenCode、DSH、Claude Code 和 Claude Desktop。覆盖值必须
大于 0，且不能超过官方清单的 `max_context_window`；清除覆盖后恢复清单中的
原始窗口（Codex 优先使用当前账号同步后的窗口）。修改后 Mixin 会同步已安装客户端，运行中的客户端需要重新加载。
Pi、OpenCode 和 DSH 接收精确窗口；Claude Code 使用其 `[1m]` 模型标记，
Claude Desktop 使用 `supports1m`，只有窗口至少为 1M 时才启用，不能表达任意
自定义窗口大小。设置窗口不会提高官方服务端实际允许的上限。

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
