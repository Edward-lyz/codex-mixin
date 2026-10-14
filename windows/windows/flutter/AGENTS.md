# Flutter 生成边界规范

继承上级规范。这里虽有版本管理文件，但由 Flutter 工具链拥有，不是手写功能目录。

- 不手改 generated_plugin_registrant 或 generated_plugins.cmake 来增加插件；从 pubspec/Flutter 命令重新生成并审查 diff。
- 保持 CMakeLists.txt 的工具生成约定，业务和窗口功能放到 Dart/runner；不在这里新增业务源码。
- 依赖插件修补通过 `windows/scripts/` 已有明确入口，不在缓存里进行不可复现的手改。
- `ephemeral/` 等纯生成缓存不建规范、不提交；本规范只约束受管理边界。
- 依赖更新验证 `flutter pub get`、`flutter analyze`、`flutter test` 与 Windows release 构建，确认注册文件可由同一输入重新生成。
