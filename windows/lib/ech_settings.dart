import 'package:flutter/material.dart';

import 'cli.dart';
import 'controller.dart';
import 'widgets.dart';

const echSettingsTitle = '启用 ECH 代理访问 GPT';

Future<void> showEchSettings(
  BuildContext context,
  MixinController controller,
) async {
  final status = await controller.runAction(
    '读取 ECH 设置', ['ech', 'status', '--json'],
  );
  if (!context.mounted) return;
  final object = decodeCliObject(status.stdout);
  if (!status.ok || object == null || object['enabled'] is! bool) {
    await showTextReport(context, echSettingsTitle,
      status.ok ? 'ECH 状态接口返回了无效 JSON' : status.output);
    return;
  }
  final enabled = object['enabled'] as bool;
  final command = await showDialog<String>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      title: const Text(echSettingsTitle),
      content: Text('当前：${enabled ? '已启用' : '未启用'}\n'
        '仅接管本地网关的官方 GPT 请求。使用 edge.1molchuan.top/plus 的 IPv4 和 ECH 配置，'
        '保留端到端 TLS。ECH 连接失败会自动关闭并回退到直连。\n\n'
        '启用会先测试连接，再保存设置并重启网关。ECH 成功不能证明中继出口位于 Azure；'
        '不影响浏览器登录或第三方 Provider。'
        '${object['fallback_reason'] == null ? '' : '\n\n已自动回退到直连：${object['fallback_reason']}'}'),
      actions: [
        TextButton(onPressed: () => Navigator.pop(dialogContext), child: const Text('取消')),
        TextButton(onPressed: () => Navigator.pop(dialogContext, 'test'), child: const Text('仅测试连接')),
        FilledButton(onPressed: () => Navigator.pop(dialogContext, enabled ? 'disable' : 'enable'),
          child: Text(enabled ? '关闭并应用' : '启用并测试')),
      ],
    ),
  );
  if (command == null || !context.mounted) return;
  final result = await runWithProgress(context, title: echSettingsTitle,
    action: (onProgress) => controller.runAction(echSettingsTitle, ['ech', command], onProgress: onProgress));
  if (!context.mounted) return;
  await showTextReport(context, echSettingsTitle, result.output);
}
