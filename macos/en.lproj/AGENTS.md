# English Localization 规范

继承上级规范。这里保存 English 的 Localizable 与 InfoPlist strings。

- 与 `macos/zh-Hans.lproj` 保持 key 集合一致，新增代码引用同时补齐两种语言。
- 文案简短具体，区分保存、应用、失败、未知和已提交后失败；不夸大操作成功。
- 保持占位符的数量、类型和顺序，遵守 `.strings` 转义；产品名、CLI 命令及专业术语保持一致。
- 不在翻译里放业务逻辑或用户真实数据；重复文案沿用现有 key。
- 执行 `./scripts/check_localizations.sh`；InfoPlist 修改额外用 `plutil -lint macos/en.lproj/InfoPlist.strings`，检查实际界面长度与截断。
