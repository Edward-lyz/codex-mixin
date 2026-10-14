# Dart 表现层规范

继承上级规范。页面/组件展示，controller 组织 UI 操作，cli 适配 CLI 进程，models 解析 contract。

- Widget 不直接执行业务命令或写配置，沿用 controller/cli；不要新增另一套 service locator 或重复状态仓库。
- JSON 在 cli/models 边界校验字段类型与错误 envelope；缺失/损坏响应显示失败，不能用空对象表示成功。
- 区分 loading、success、failure 和 unknown；保留已提交后失败的提示。操作结束刷新真实状态，不能靠乐观本地值永久覆盖。
- async UI 更新检查 mounted/生命周期，dispose 清理 listener/timer/controller；窗口间状态同步沿用现有机制。
- UI 线程不阻塞等待子进程，stdout/stderr 保持分离；命令参数用列表，密钥走环境引用并脱敏日志。
- 使用已有 Flutter/Dart 类型与依赖，字段默认 private，遵守 analysis_options；表单覆盖键盘、焦点、语义标签与深浅色。
- 在 `windows/` 执行 `dart format --output=none --set-exit-if-changed lib test`、`flutter analyze`、`flutter test`，业务 contract 改动同时验证 Rust CLI。
