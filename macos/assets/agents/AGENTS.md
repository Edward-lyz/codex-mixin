# Agent 品牌资源规范

继承上级规范。`manifest.json` 是固定 Agent ID、资源路径与 template-color 的映射，`ATTRIBUTION.md` 记录来源。

- 同步 application 固定 ID 和两端 GUI，缺资源明确失败，不用无关品牌 logo 冒充。
- fork 复用父品牌时保留明确映射；彩色与 monochrome/template 用法按 manifest 表达，不能统一强制染色。
- PNG 品牌保留原图，派生 SVG 容器不能声称是矢量重建；来源或版权更新同步 attribution。
- SVG 不含活动内容/远程依赖，大小按菜单/窗口用途控制；不要提交下载缓存。
- 运行 `cargo test --locked --test agent_logos`、`./scripts/test_agent_logos.sh`；涉及 Windows 消费时运行其 agent_logo_test。
