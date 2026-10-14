# 简体中文 Localization 规范

继承上级规范。这里保存简体中文的 Localizable 与 InfoPlist strings。

- 与 `macos/en.lproj` 保持 key 集合一致，新增文案同一提交更新两种语言。
- 使用完整短句，准确区分保存、应用、错误、未知和提交后的部分失败；专业术语与产品名遵守现有约定。
- 保留占位符数量、类型与顺序，正确转义引号/换行；不能在中文字符串中编码业务判断。
- 不提交个人目录、真实 key/token 或用户配置。
- 执行 `./scripts/check_localizations.sh`；InfoPlist 修改额外用 `plutil -lint macos/zh-Hans.lproj/InfoPlist.strings`，实际检查布局与截断。
