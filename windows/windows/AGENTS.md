# Flutter Windows Host 规范

继承上级规范。这里是 Flutter 约定的原生工程，目录名不能随意修改。

- 顶层 CMake 组织嵌入与资源；`runner` 管理窗口/进程入口；`flutter` 为工具链生成边界。
- C++ Host 不实现 provider、配置、认证或网关逻辑，业务经 Dart/CLI contract 完成。
- 保持 Flutter 插件注册与多窗口约定，原生资源生命周期和错误回传明确。
- 不提交 build、ephemeral、编译缓存；生成文件通过 Flutter/依赖入口更新。
- CMake、manifest、图标或 runner 修改在 Windows 执行 `flutter build windows --release` 与打包验证，其他主机只报告可完成的静态检查。
