# 多 Agent Schema 适配规范

继承上级规范。`schemas.rs` 声明经来源核验的格式与能力，`engine.rs` 执行托管字段读写，`mod.rs` 暴露适配入口。

- 新 schema 需注明可核验的客户端版本/源码依据，区分 CLI、桌面与 fork 的真实协议限制。
- 固定 Agent ID 与 application、资源 manifest、CLI contract 一致；不能只新增图标就宣称支持接入。
- 格式差异由明确 schema 表达；仅共同变化才复用 engine，不以通用字段覆盖不同客户端语义。
- 备份精确到托管字段；用户已修改字段时断开报告冲突，不能静默恢复整份旧配置。
- 不支持的操作返回明确 unavailable，不写成功占位文件；未知客户端不猜测目录或键名。
- 对每类 schema 验证实际文件接入/断开和冲突路径，运行 frameworks/agents 相关 Rust 测试与根级门禁。
