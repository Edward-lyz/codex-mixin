# Provider 品牌资源规范

继承上级规范。这里只保存 provider 展示图标及来源，不决定 provider 支持能力。

- 图标名称与 ProviderSupport/资源生成脚本约定一致；自定义 provider 使用已有 custom 语义。
- 更新图标同时检查 macOS 图标缓存与 Windows 生成入口，不能直接修改 Windows 忽略的派生文件。
- 保持品牌原貌、浅深色可读和 attribution；SVG 禁止脚本与外部依赖。
- 不在图标或文件名嵌入 endpoint、密钥或用户配置。
- 执行 `./scripts/test_provider_support.sh`，验证 manifest/消费者引用及实际显示；Windows 用生成脚本与 Flutter 检查验证派生结果。
