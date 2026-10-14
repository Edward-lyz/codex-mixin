# Provider 领域规范

继承上级规范。这里拥有定义、preset、模型身份、路由元数据、能力、发现与认证适配。

- `types`/`spec` 表达规则，`registry`/`resolver` 解析身份与元数据；网络 I/O 限于既有 discovery、quota、auth、probe 等能力边界。
- 不承担入站服务器状态、CLI 交互或网关执行编排；provider 规则不反向引用网关层。
- 模型 key、catalog slug、上游 ID 与来源分别保存，不通过字符串猜测混用；保留模型级端点与认证策略。
- preset 的特殊行为必须有真实接口依据，不按 provider 名称堆叠无验证猜测。
- 能力未知和探测失败不等于不支持；配置身份变化后旧缓存必须失效。
- 验证定义边界、模型端点、认证、发现合并与缓存身份，运行 provider 相关测试、`cargo test --locked --test provider_probe_cache` 和根级门禁。
