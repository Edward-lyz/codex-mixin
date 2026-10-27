# 上下文引擎展示与 Wiki 计划

状态：2026-10-27，实施中。用户要求进一步减少图中文字，用图形表达；本 chat 负责 SVG/Wiki，另一个 chat 负责 Messages 接入代码。

本轮调整：重绘现有 SVG，采用三行并列对比、历史块到摘要的形变、独立归档和恢复箭头；将配置、边界与来源解释移至 Wiki。同步 Wiki 页面、资源、首页和侧栏。风险：不能用图形暗示原生产品删除日志或 Mixin 摘要无损；Messages 新支持在实现验收前仍标注为待验证。

## 当前行为

- `docs/context-engine.md` 已解释 Rust 上下文引擎，但缺少直观对比图；GitHub Wiki 尚无独立 Context Engine 页面。
- 实现基线为 `f1e24cc50a816e1f722d08cf0f895c61a49b9ebf`。引擎默认关闭；启用后模型选择连续片段写摘要，Rust 校验并保存原文与工作视图，支持三级压缩、检索和恢复。
- Billion Context 提供触发决策与提示规则来源；不是完整 TypeScript kernel 的移植，也不能把其性能数据当成本项目结果。

## 文件与预期变化

1. 新增 `docs/assets/context-engine-comparison.svg`：科研风格的机制对比，分别展示 Codex、Claude Code 与 Mixin，注明图示非性能测量。
2. 新增 `docs/wiki/Context-Engine.md`：可用于 GitHub Wiki 的中文页面，说明作用、来源、适用边界、配置、检查方法和故障排查。
3. 修改 `docs/context-engine.md`、`README.md`、`README_en.md`：加入图与用户指南入口。
4. 在允许 repo 操作的 `PUBLIC_REPO/codex-mixin.wiki` checkout 添加页面及资源，更新 `Home.md` 与 `_Sidebar.md`。现有 PRIVATE 归档目录只读，不在其中开发。

## 实施顺序

核对代码与官方资料 → 编写 SVG 与 Wiki → 渲染并人工查看图 → 检查 XML、链接、CLI 参数、diff 与架构门禁 → 按仓库分别提交最小文档闭包。

## 风险与边界

- Codex 的本地摘要、官方远端 compaction 与 Claude Code 的压缩不是同一实现；不得合并成一个虚构算法。
- 模型可见历史被替换不等于磁盘记录删除；原文可恢复不等于摘要无损或模型一定主动恢复。
- checkpoint 接管针对 Responses compact/compaction_trigger；不据此宣称接管所有客户端的原生 compact。
- 不展示未经当前版本验证的压缩率、准确率或时延优势；现有小样本实验不是 Codex/CC 横向基准。
- 配置写入会重启运行中的网关，本任务只校验 help/源码，不修改用户运行配置。
- 不修改 Rust/GUI，不新增依赖或自定义代码抽象，不运行无关完整编译；检查 `git diff --check`、本地链接、命令存在性及 `scripts/check_arch.sh`。
- 不推送或发布，除非用户明确要求；最终标明本地提交与发布状态。

## 验收结果

- 完成低文字量三行 SVG，人工检查完整静态渲染；XML 校验通过。
- 完成 Wiki 内容与独立 Wiki 首页、侧栏、资源同步；本地链接检查通过。
- 两仓库 diff 空白检查、架构检查通过。文档修改未运行 Rust 运行时测试。
- 原生 Messages 接入由独立会话开发，尚未验收；文档按已核对基线说明边界。
- 仅本地提交，不推送。
