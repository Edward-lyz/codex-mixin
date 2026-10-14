# NASA 装饰图片规范

继承上级规范。这些图片用于安装卡装饰，`manifest.json` 和 `ATTRIBUTION.md` 记录本地资源与来源。

- 更新使用 `scripts/sync_nasa_wallpapers.py`，先检查其参数/下载结果；不得把下载网页或无效文件当作图片提交。
- manifest 的路径、日期、来源和现有元数据与真实文件一致；尺寸/体积按安装卡用途控制。
- 保持已提交可用集合。刷新失败沿用构建脚本明确的装饰资源降级，不能推广为业务成功兜底。
- 不手改生成元数据以掩盖缺图；新增来源同步 attribution。
- 验证图片可解码、manifest 引用存在，执行 `./scripts/test_install_card.sh` 并实际检查卡片显示。
