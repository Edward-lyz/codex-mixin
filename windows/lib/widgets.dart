import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';

import 'models.dart';
import 'theme.dart';

Widget providerLogo(
  ProviderModel provider,
  double size, {
  bool selected = false,
}) => Container(
  width: size,
  height: size,
  padding: EdgeInsets.all(size * .16),
  decoration: BoxDecoration(
    color: selected ? Colors.white.withValues(alpha: .16) : Colors.white,
    borderRadius: BorderRadius.circular(size * .23),
  ),
  child: SvgPicture.asset(
    'assets/providers/${provider.icon}.svg',
    colorFilter: ColorFilter.mode(
      providerIconColor(provider.icon),
      BlendMode.srcIn,
    ),
    placeholderBuilder: (_) => Icon(
      Icons.dns_outlined,
      color: selected ? Colors.white : muted,
      size: size * .5,
    ),
  ),
);

class MixinTitleBar extends StatelessWidget {
  final String title;
  final String? status;
  final VoidCallback onClose;
  final VoidCallback? onMinimize;
  final VoidCallback? onMaximize;
  final Color? statusColor;
  final Widget? titleTrailing;
  final Widget? trailing;
  final Widget? leading;

  const MixinTitleBar({
    super.key,
    required this.title,
    required this.onClose,
    this.status,
    this.onMinimize,
    this.onMaximize,
    this.statusColor,
    this.titleTrailing,
    this.trailing,
    this.leading,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 52,
      color: Colors.white,
      child: Row(
        children: [
          const SizedBox(width: 10),
          ?leading,
          const SizedBox(width: 8),
          Text(
            title,
            style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
          ),
          ?titleTrailing,
          Expanded(
            child: status?.isNotEmpty == true
                ? Padding(
                    padding: const EdgeInsets.only(left: 16),
                    child: Text(
                      status!,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: statusColor ?? muted,
                        fontSize: 11,
                      ),
                    ),
                  )
                : const SizedBox.shrink(),
          ),
          ?trailing,
          // Traditional Windows caption buttons, flush to the top-right corner.
          if (onMinimize != null)
            _CaptionButton(
              icon: Icons.remove,
              tooltip: '最小化',
              onTap: onMinimize!,
            ),
          if (onMaximize != null)
            _CaptionButton(
              icon: Icons.crop_square,
              tooltip: '最大化/还原',
              onTap: onMaximize!,
            ),
          _CaptionButton(
            icon: Icons.close,
            tooltip: '关闭',
            onTap: onClose,
            isClose: true,
          ),
        ],
      ),
    );
  }
}

class _CaptionButton extends StatefulWidget {
  final IconData icon;
  final String tooltip;
  final VoidCallback onTap;
  final bool isClose;
  const _CaptionButton({
    required this.icon,
    required this.tooltip,
    required this.onTap,
    this.isClose = false,
  });

  @override
  State<_CaptionButton> createState() => _CaptionButtonState();
}

class _CaptionButtonState extends State<_CaptionButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final background = _hover
        ? (widget.isClose ? const Color(0xffe81123) : const Color(0x14000000))
        : Colors.transparent;
    final iconColor = _hover && widget.isClose
        ? Colors.white
        : const Color(0xff3f4247);
    return Tooltip(
      message: widget.tooltip,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Container(
            width: 46,
            height: 52,
            alignment: Alignment.center,
            color: background,
            child: Icon(widget.icon, size: 16, color: iconColor),
          ),
        ),
      ),
    );
  }
}

/// Flat icon button: no filled/circular background, just the icon with a
/// subtle rounded hover highlight. Used for lightweight title-bar actions.
class FlatIconButton extends StatefulWidget {
  final IconData icon;
  final String tooltip;
  final VoidCallback onTap;
  final double size;
  const FlatIconButton({
    super.key,
    required this.icon,
    required this.tooltip,
    required this.onTap,
    this.size = 22,
  });

  @override
  State<FlatIconButton> createState() => _FlatIconButtonState();
}

class _FlatIconButtonState extends State<FlatIconButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: widget.tooltip,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Container(
            padding: const EdgeInsets.all(6),
            decoration: BoxDecoration(
              color: _hover ? const Color(0x14000000) : Colors.transparent,
              borderRadius: BorderRadius.circular(8),
            ),
            child: Icon(
              widget.icon,
              size: widget.size,
              color: const Color(0xff3f4247),
            ),
          ),
        ),
      ),
    );
  }
}

/// A tappable surface with reliable hover/press feedback.
///
/// The app theme disables Material ink globally (`NoSplash` + transparent
/// hover/highlight colors), so `InkWell` shows no hover state inside the
/// frameless tray popup. This widget paints its own rounded background that
/// darkens on hover and further on press, driven by `MouseRegion` + `setState`
/// (the same mechanism the title-bar caption buttons use).
class HoverButton extends StatefulWidget {
  final Widget child;
  final VoidCallback onTap;
  final EdgeInsetsGeometry padding;
  final double borderRadius;
  final Color baseColor;
  final Color borderColor;
  final double? width;

  const HoverButton({
    super.key,
    required this.child,
    required this.onTap,
    this.padding = EdgeInsets.zero,
    this.borderRadius = 9,
    this.baseColor = Colors.transparent,
    this.borderColor = Colors.transparent,
    this.width,
  });

  @override
  State<HoverButton> createState() => _HoverButtonState();
}

class _HoverButtonState extends State<HoverButton> {
  bool _hover = false;
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    final overlay = _pressed
        ? const Color(0x24000000)
        : _hover
        ? const Color(0x14000000)
        : Colors.transparent;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() {
        _hover = false;
        _pressed = false;
      }),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTapDown: (_) => setState(() => _pressed = true),
        onTapUp: (_) => setState(() => _pressed = false),
        onTapCancel: () => setState(() => _pressed = false),
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          width: widget.width,
          padding: widget.padding,
          decoration: BoxDecoration(
            color: Color.alphaBlend(overlay, widget.baseColor),
            border: Border.all(color: widget.borderColor),
            borderRadius: BorderRadius.circular(widget.borderRadius),
          ),
          child: widget.child,
        ),
      ),
    );
  }
}

Future<void> showTextReport(BuildContext context, String title, String report) {
  return showDialog<void>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(title, textAlign: TextAlign.center),
      content: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 520, maxHeight: 340),
        child: Container(
          width: 480,
          padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
          decoration: BoxDecoration(
            color: const Color(0xfff6f7f9),
            borderRadius: BorderRadius.circular(10),
          ),
          child: SingleChildScrollView(
            child: SelectableText(
              formatCliReport(report),
              style: const TextStyle(fontSize: 13, height: 1.55),
            ),
          ),
        ),
      ),
      actionsAlignment: MainAxisAlignment.center,
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('关闭'),
        ),
      ],
    ),
  );
}

Future<T> runWithProgress<T>(
  BuildContext context, {
  required String title,
  required Future<T> Function(void Function(String line) onProgress) action,
  String Function(T result)? resultText,
}) async {
  var dialogOpen = true;
  final completed = ValueNotifier<bool>(false);
  final progress = ValueNotifier<String>('$title处理中，请稍候...');
  final dialogFuture = showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (_) => ValueListenableBuilder<bool>(
      valueListenable: completed,
      builder: (_, isCompleted, _) => AlertDialog(
        title: Text(title, textAlign: TextAlign.center),
        content: SizedBox(
          width: 300,
          child: ValueListenableBuilder<String>(
            valueListenable: progress,
            builder: (_, message, _) => Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (!isCompleted) ...[
                  const SizedBox(
                    width: 28,
                    height: 28,
                    child: CircularProgressIndicator(strokeWidth: 2.6),
                  ),
                  const SizedBox(height: 16),
                ],
                Text(message, textAlign: TextAlign.center),
              ],
            ),
          ),
        ),
        actionsAlignment: MainAxisAlignment.center,
        actions: [
          if (isCompleted)
            TextButton(
              onPressed: () {
                dialogOpen = false;
                Navigator.pop(context);
              },
              child: const Text('确认'),
            ),
        ],
      ),
    ),
  );
  // Let Flutter push the modal route before starting the operation. This
  // prevents a fast CLI call from racing the dialog creation.
  await Future<void>.delayed(Duration.zero);
  final actionFuture = action((line) {
    final message = formatCliProgress(line);
    if (message.isNotEmpty) progress.value = message;
  });
  try {
    final result = await actionFuture;
    // Always surface the outcome inside this dialog when the caller wants a
    // result shown; the user dismisses it with 确认. Setting the notifiers does
    // not depend on `context`, so a background refresh that rebuilds the page
    // can never make the result disappear.
    if (resultText != null) {
      progress.value = resultText(result);
      completed.value = true;
      await dialogFuture;
    }
    return result;
  } catch (error) {
    // Never close silently on an error: show it in the same dialog so the user
    // always sees why an operation failed.
    progress.value = '$title失败：$error';
    completed.value = true;
    await dialogFuture;
    rethrow;
  } finally {
    // Auto-close path (no explicit result requested and no error). The 确认
    // button already popped the dialog in the branches above.
    if (dialogOpen && context.mounted) {
      final navigator = Navigator.of(context, rootNavigator: true);
      if (navigator.canPop()) navigator.pop();
    }
    dialogOpen = false;
    progress.dispose();
    completed.dispose();
  }
}

String formatCliProgress(String line) {
  final formatted = formatCliReport(line).trim();
  return formatted.isEmpty ? '正在处理中，请稍候...' : formatted;
}

Future<bool> confirmAction(
  BuildContext context, {
  required String title,
  required String message,
  String confirmLabel = '确定',
}) async {
  return await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: Text(title, textAlign: TextAlign.center),
          content: Text(message, textAlign: TextAlign.center),
          actionsAlignment: MainAxisAlignment.center,
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: Text(confirmLabel),
            ),
          ],
        ),
      ) ??
      false;
}

String formatCliReport(String report) {
  final lines = report
      .split(RegExp(r'\r?\n'))
      .map((line) {
        var text = line.trimRight();
        if (text.startsWith('MIXIN_PROGRESS ')) {
          text = text.substring('MIXIN_PROGRESS '.length);
        }
        if (text.contains('Codex config is not managed by codex-mixin')) {
          return '当前 Codex 配置还没有被 Codex Mixin 接管，需要先「安装到 Codex」。';
        }
        if (text.contains('failed to load Codex config') ||
            text.contains('config.load check failed')) {
          final reason = text.contains('config.load check failed:')
              ? text.split('config.load check failed:').last.trim()
              : '';
          return reason.isEmpty
              ? 'Codex 拒绝加载托管配置。已回滚本次安装，请检查 ~/.codex/config.toml 后重试。'
              : 'Codex 拒绝加载托管配置（$reason）。已回滚本次安装，请检查 ~/.codex/config.toml 后重试。';
        }
        if (text.contains(
          'no enabled Baidu reporting provider is configured',
        )) {
          return '没有启用百度代码上报的供应商，请在供应商设置中开启「百度代码上报」后重试。';
        }
        if (text.contains('official mode requires an existing Codex login')) {
          return '「官方账号模式」需要先登录官方 Codex。请先在 Codex 中完成 `codex login` 登录，'
              '或改用「仅自定义模型模式」后重试。';
        }
        final localized = _localizeCliLine(text);
        return localized ?? text;
      })
      .where((line) => line.trim().isNotEmpty)
      .toList();
  return lines.join('\n');
}

/// Best-effort Simplified-Chinese rendering of the gateway/install CLI output so
/// the report dialogs contain no English. Internal step/trace lines are dropped;
/// progress stages and summary lines are translated. Unknown lines fall through
/// to the caller (returned as-is).
String? _localizeCliLine(String text) {
  // Progress stage names (MIXIN_PROGRESS prefix already stripped by the caller).
  const stages = <String, String>{
    'Checking local config and gateway state': '正在检查本地配置与网关状态',
    'Loading Codex config template': '正在加载 Codex 配置模板',
    'Fetching available models': '正在获取可用模型',
    'Loading model metadata': '正在加载模型元数据',
    'Preparing or installing Codex CLI': '正在准备/安装 Codex CLI',
    'Writing Codex config and model catalog': '正在写入 Codex 配置与模型目录',
    'Syncing history sessions and SQLite state': '正在同步历史会话与本地数据',
    'Validating install result': '正在校验安装结果',
    'Reading and locking Codex config': '正在读取并锁定 Codex 配置',
    'Restoring pre-install config and auth state': '正在恢复安装前的配置与认证状态',
    'Restoring history sessions and SQLite state': '正在恢复历史会话与本地数据',
    'Preparing DUCX authentication': '正在准备 DUCX 身份认证',
    'Downloading DUCX authentication package': '正在下载 DUCX 身份认证组件',
    'Installing DUCX authentication package': '正在安装 DUCX 身份认证组件',
    'Preparing DUCX data-report': '正在准备 DUCX 额度上报',
    'Waiting for DUCX login': '正在等待 DUCX 登录',
    'DUCX login is required': '需要登录 DUCX',
    'DUCX authentication completed.': 'DUCX 身份认证已完成',
    'Managed DUCX authentication is ready': '托管 DUCX 身份认证已就绪',
    'Managed DUCX installed': '托管 DUCX 已安装',
  };
  if (stages.containsKey(text)) return stages[text];
  final ducxDownload = RegExp(
    r'^Downloading DUCX (\d+)(?:/(\d+))? MiB$',
  ).firstMatch(text);
  if (ducxDownload != null) {
    final total = ducxDownload.group(2);
    return total == null
        ? '正在下载 DUCX：${ducxDownload.group(1)} MiB'
        : '正在下载 DUCX：${ducxDownload.group(1)} / $total MiB';
  }

  // Internal developer step/trace lines — hide from the user-facing report.
  if (text.startsWith('codex install step:') ||
      text.startsWith('codex install started:') ||
      text.startsWith('codex validation started:') ||
      text.startsWith('metadata entries loaded:') ||
      text.startsWith('web search discovery:') ||
      text.startsWith('web search capabilities:')) {
    return '';
  }

  // Summary lines.
  if (text.startsWith('codex validation: doctor config.load ok')) {
    return '校验通过：Codex 已成功加载托管配置';
  }
  final models = RegExp(r'debug models loaded (\d+) models').firstMatch(text);
  if (models != null) return '已加载 ${models.group(1)} 个模型';
  // Manual report ("手动触发上报") summary lines.
  final queued = RegExp(
    r'^DUCX reports queued from local sessions:\s*(\d+)',
  ).firstMatch(text);
  if (queued != null) return '已从本地会话收集 ${queued.group(1)} 条上报';
  final delivered = RegExp(
    r'^DUCX reports delivered:\s*(\d+)',
  ).firstMatch(text);
  if (delivered != null) return '已成功上报 ${delivered.group(1)} 条';
  if (text == 'codex install step: validation completed' ||
      text == 'validation completed') {
    return '安装校验完成';
  }
  final installed = RegExp(r'^models installed:\s*(\d+)').firstMatch(text);
  if (installed != null) return '已安装模型：${installed.group(1)} 个';
  if (text.startsWith('codex config updated:')) return '已更新 Codex 配置';
  if (text.startsWith('codex config backup:')) return '已备份原 Codex 配置';
  if (text.startsWith('model catalog written:')) return '已写入模型目录';
  if (text.startsWith('provider:')) {
    return '供应商：${text.substring('provider:'.length).trim()}';
  }
  if (text.startsWith('base_url:')) {
    return '本地接口地址：${text.substring('base_url:'.length).trim()}';
  }
  if (text.startsWith('default model:')) {
    final value = text.substring('default model:'.length).trim();
    return value == 'unchanged' ? '默认模型：保持不变' : '默认模型：$value';
  }
  final history = RegExp(
    r'history migrated:\s*(\d+) JSONL files,\s*(\d+) SQLite rows',
  ).firstMatch(text);
  if (history != null) {
    return '历史迁移：${history.group(1)} 个会话文件，${history.group(2)} 条记录';
  }
  if (text.startsWith('reload required:')) {
    return '需要重启 Codex（应用需重启；CLI 请新开会话）后生效';
  }
  if (text.contains('custom-only model picker')) {
    return '已启用自定义模型选择（本地占位登录，不连接 AWS）';
  }
  if (text.contains(
    'official Codex plugins, cloud tasks, and account features',
  )) {
    return '官方 Codex 插件、云任务与账号功能：不可用';
  }
  if (text.contains('managed DUCX installation is not supported on Windows')) {
    return 'Windows 当前没有可用的托管 DUCX，请先安装 Windows DUCX 或填写可执行文件路径';
  }
  if (text.contains('DUCX authentication is not supported on this platform')) {
    return '当前 Windows 版本暂不支持 DUCX 身份认证';
  }
  if (text.contains('DUCX reporting is not supported on this platform')) {
    return '当前 Windows 版本暂不支持 DUCX 额度上报';
  }
  if (text.contains('DUCX login is required')) {
    return '需要登录百度 DUCX：请在弹出的 DUCX 窗口完成登录后重新保存';
  }
  if (text.contains('is installed but not logged in')) {
    return '百度 DUCX 已安装但尚未登录：请在弹出的 DUCX 窗口完成登录后重新保存';
  }
  if (text.contains('failed to open the DUCX login window')) {
    return '无法打开 DUCX 登录窗口，请重试';
  }
  if (text.startsWith('Error: ')) return text.substring('Error: '.length);
  return null;
}
