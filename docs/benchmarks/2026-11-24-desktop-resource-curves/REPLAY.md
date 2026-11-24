# 复跑桌面资源横评

本目录的回放驱动与采样器可复用于 macOS 上的三产品横评。先准备三个**独立配置、独立进程 coalition** 的完整桌面应用及其本地网关，再填写 PID、端口和模型；驱动不负责安装、配置或启动产品。

## 环境与配置

1. 准备独立临时目录和各产品单独的 `HOME`、`CODEX_HOME`、`XDG_CONFIG_HOME`、`XDG_CACHE_HOME`；Mixin 还需隔离 `CODEX_GATEWAY_CONFIG`、`CODEX_GATEWAY_RUNTIME_FILE`、`CODEX_GATEWAY_DAEMON_FILE`、`CODEX_GATEWAY_SERVICE_ID` 和 `CODEX_MIXIN_CONTEXT_DB`。CC Switch 还需隔离 `CC_SWITCH_TEST_HOME` 和 `CLAUDE_CONFIG_DIR`。保留现有应用及其目录，不复用它们的网关、数据库或服务 ID。
2. 三个网关都配置 synthetic key `bench-key` 和指向 `http://127.0.0.1:45671/v1` 的 Responses 上游。Mixin 的完整上游地址为 `http://127.0.0.1:45671/v1/responses`。分别启动并确认三个网关只监听各自的 loopback 端口；当前报告使用 Mixin `8149`、Magpie `45673`、CC Switch `45674`。回放驱动启动后才监听上游端口，启动产品时上游尚未监听是正常的。
3. 本次 Magpie 使用模型 `benchmark-model`、provider ID `bench`，网关模型名为 `bench/benchmark-model`；CC Switch 使用 Codex provider `bench`、模型 `benchmark-model` 和 proxy 路由；Mixin 使用 provider `bench`、模型 `benchmark-model`，网关模型名 `benchmark-model-bench`。将上下文窗口设置为至少 1,048,576。实际使用不同名称时应同步修改 spec。
   Mixin 在启动时可能发现本机已有的 Claude Code 登录；如隔离配置中出现自动发现的 `claude-code`，应使用隔离环境中的 `codex-mixin provider remove claude-code --no-tui` 移除，并确认启动日志中的供应商仅包含 `bench`。该操作会在隔离目录记录忽略项，避免后续自动重新添加；不要操作用户的真实配置。
4. 使用独立应用副本，通过 Launch Services 启动，以便系统为各应用创建独立 coalition。本次为副本设置不同的 bundle identifier、使用继承隔离环境的启动包装器并对修改后的副本重新 ad-hoc 签名。Mixin 的桌面进程和其启动的 gateway 都必须属于正确的隔离实例。直接从同一 shell 启动可能继承同一 coalition，不能用于本方法。
5. 加载完整桌面界面，统一隐藏窗口，保持网关和界面进程存活；确保没有其他程序监听所选端口。记录每个产品的 UI PID 和实际处理请求的 gateway PID，以及必须排除的现有生产 PID。

轻量模式对比时，应先启用各产品的轻量模式并关闭主窗口。Magpie 需要等待日志出现 `lightweight: the main window's webview let go`，再检查其 coalition 中 `com.apple.WebKit.WebContent` 和 `com.apple.WebKit.GPU` 均已退出。GPU 进程可能晚于 WebView 释放一分多钟；连续多次检查均不存在后再启动回放。回放完成后还需检查整个采样文件，确认这两个进程没有在测量窗口内重现。仅等待固定时间或仅检查 WebContent 不足以确认该状态。

## 编译与运行

在仓库根目录执行，使用实际绝对路径替换占位符；输出目录须为新目录：

```sh
clang -O2 -Wall -Wextra -o /tmp/measure-desktop-processes scripts/measure_desktop_processes.c
node --check scripts/measure_desktop_resources.mjs
# 轻量模式：重复检查 Magpie coalition 成员，直到 WebContent 和 GPU 都退出
/tmp/measure-desktop-processes 0 MAGPIE_PID MAGPIE_PID | jq -r '.products[0].members[].name'
cp docs/benchmarks/2026-11-24-desktop-resource-curves/replay-spec.example.json /tmp/desktop-replay-spec.json
# 编辑 /tmp/desktop-replay-spec.json，填写绝对路径、端口、模型及真实 PID
node scripts/measure_desktop_resources.mjs /tmp/desktop-replay-spec.json
```

`corpus` 和 `images` 可指向仓库中 `docs/benchmarks/2026-11-06-mixin-magpie-1m/` 的 `corpus.json.gz`、`fixture-1.png`、`fixture-2.png`。`sampler` 填编译后的可执行文件绝对路径；`output` 为新输出目录；`protectedPids` 填生产进程 PID。`allowedExtraMessages` 是允许网关注入的消息数，本次 Mixin 为 1，其余为 0。驱动用 `lsof` 核对 gateway PID 与监听端口，要求同一产品的 UI 与 gateway 在同一 coalition，并拒绝共享产品 coalition、混入的保护进程、驱动及其祖先进程或采样器。轻量模式没有 UI 时，`uiPid` 和 `gatewayPid` 均填网关 PID；本次 Mixin 使用独立 launchd 服务启动网关以获得独立 coalition。

默认 `levels` 为 `[0,2,4,8]`，每个模拟主会话在首个输出后启动四条子会话请求，即每产品 0/10/20/40 条并发 HTTP 流。驱动使用 `timing` 中可选的毫秒字段 `warmup`、`settle`、`window`、`gap`、`interval` 和窗口数 `windows`；默认分别为 20000、10000、20000、5000、250 和 3。上游固定每 250 毫秒发送一次增量，与采样间隔独立；采样器每轮扫描后等待指定间隔，实际周期还包括扫描耗时。每个测量窗口检查每条客户端流是否持续收到数据。正式对比应保留相同工作负载与采样口径。会话名称只用于组织请求，并不启动实际的 agent 子进程。

2026-11-24 已保存的数据由加入上述逐窗口流进度检查、PID/端口核对和逐进程 CPU 时间戳前的版本采集；这些后续改进不改变历史数据。历史运行已校验每条请求首个增量、完整响应和每档加载后的上游活跃数；窗口内提前断流会使运行失败。

完成后检查 `results.json` 中各档活跃数、请求完成数、`validation.errors` 与 `finalActive`；检查 `samples.jsonl.gz` 中 UI、gateway、helper 和 coalition 的归属。每档结果是三个连续 20 秒窗口的中位数，单次升压过程中历史和缓存会累积。按 PID 停止本次创建的实例，删除本次临时配置与安装包，保留结果、采样数据与脚本。
