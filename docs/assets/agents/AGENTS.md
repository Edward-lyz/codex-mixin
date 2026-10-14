# README Agent Logo 派生规范

继承上级规范。这里存放从 `macos/assets/agents/` 派生的独立 SVG，来源约定见本目录 README。

- 彩色标识优先直接复用源资源；monochrome 派生显式固定 light/dark 颜色，适配独立图片环境。
- PNG 的 SVG 容器只嵌入原始 PNG，不追踪/重绘后声称是官方矢量标识。
- 资源 ID、README 引用与 macOS manifest 保持一致；变更注明派生来源，不成为新的品牌事实源。
- 保留 attribution 路径，SVG 禁止活动内容或远程依赖；不包含用户数据。
- 检查源图、嵌入数据、链接和浅深色实际渲染，运行相关 logo 测试。
