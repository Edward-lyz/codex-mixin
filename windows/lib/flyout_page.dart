import 'dart:async';
import 'dart:convert';

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'controller.dart';
import 'log.dart';
import 'theme.dart';
import 'widgets.dart';

class FlyoutPage extends StatefulWidget {
  final MixinController controller;
  final int windowId;
  final String title;
  final int trayWindowId;

  const FlyoutPage({
    super.key,
    required this.controller,
    required this.windowId,
    required this.title,
    required this.trayWindowId,
  });

  @override
  State<FlyoutPage> createState() => _FlyoutPageState();
}

class _FlyoutPageState extends State<FlyoutPage> with WindowListener {
  bool _busy = false;
  bool _everFocused = false;
  DateTime _ignoreBlurUntil = DateTime.fromMillisecondsSinceEpoch(0);
  Timer? _focusWatch;

  static const _installActions = [
    ('安装到 Codex...', Icons.file_download_outlined),
    ('从 Codex 恢复...', Icons.restore),
    ('安装到 Claude Code...', Icons.file_download_outlined),
    ('从 Claude Code 恢复...', Icons.restore),
    ('安装到 DSH...', Icons.file_download_outlined),
    ('从 DSH 卸载...', Icons.restore),
    ('安装到 OpenCode...', Icons.file_download_outlined),
    ('从 OpenCode 卸载...', Icons.restore),
    ('安装到 Pi...', Icons.file_download_outlined),
    ('从 Pi 卸载...', Icons.restore),
  ];

  @override
  void initState() {
    super.initState();
    windowManager.addListener(this);
    // A frameless always-on-top window can emit a spurious blur right after it
    // appears; ignore blur briefly and grab focus so click-away dismissal works.
    _ignoreBlurUntil = DateTime.now().add(const Duration(milliseconds: 500));
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      try {
        await windowManager.show();
        await windowManager.focus();
      } catch (_) {}
    });
    // Blur events are not always delivered to sub-windows; poll focus as a
    // backup so an outside click still dismisses the submenu. Only auto-close
    // once the window has actually held focus, to avoid closing on open.
    _focusWatch = Timer.periodic(const Duration(milliseconds: 250), (_) async {
      if (DateTime.now().isBefore(_ignoreBlurUntil)) return;
      bool focused;
      try {
        focused = await windowManager.isFocused();
      } catch (_) {
        return;
      }
      if (focused) {
        _everFocused = true;
      } else if (_everFocused) {
        _focusWatch?.cancel();
        unawaited(windowManager.hide());
      }
    });
  }

  @override
  void dispose() {
    _focusWatch?.cancel();
    windowManager.removeListener(this);
    super.dispose();
  }

  @override
  void onWindowFocus() {
    _everFocused = true;
  }

  @override
  void onWindowBlur() {
    if (DateTime.now().isBefore(_ignoreBlurUntil)) return;
    // Clicking away only dismisses the submenu itself; the tray popup stays so
    // opening a submenu never makes the tray disappear unexpectedly.
    _focusWatch?.cancel();
    unawaited(windowManager.hide());
  }

  Future<void> _close() async {
    _focusWatch?.cancel();
    await windowManager.hide();
    if (widget.trayWindowId > 0) {
      try {
        await WindowController.fromWindowId(widget.trayWindowId).hide();
      } catch (_) {}
    }
  }

  Future<void> _sendMain(String method, [Map<String, dynamic>? extra]) async {
    UiLog.instance.info(
      'flyout _sendMain method=$method (title=${widget.title})',
    );
    await _close();
    try {
      await DesktopMultiWindow.invokeMethod(
        0,
        'command',
        jsonEncode({'method': method, ...?extra}),
      );
      UiLog.instance.info('flyout _sendMain invoked ok method=$method');
    } catch (e, s) {
      UiLog.instance.error('flyout _sendMain failed method=$method', e, s);
    }
  }

  Future<void> _runClient(
    String title,
    List<String> args, {
    bool confirmRestore = false,
  }) async {
    if (confirmRestore) {
      final confirmed = await confirmAction(
        context,
        title: title,
        message: '会恢复安装前备份的配置。',
      );
      if (!confirmed) return;
    }
    setState(() => _busy = true);
    if (!mounted) return;
    await runWithProgress(
      context,
      title: title,
      resultText: (result) => result.ok
          ? '$title成功'
          : '$title失败：${formatCliReport(result.output).trim().isEmpty ? '未返回具体原因' : formatCliReport(result.output).trim()}',
      action: (onProgress) =>
          widget.controller.runAction(title, args, onProgress: onProgress),
    );
    if (!mounted) return;
    setState(() => _busy = false);
    await _close();
  }

  Future<void> _runAdvanced(String label) async {
    setState(() => _busy = true);
    final result = await runWithProgress(
      context,
      title: label,
      action: (_) => label == '手动触发上报...'
          ? widget.controller.reportReplay()
          : widget.controller.exportConfig(),
    );
    if (!mounted) return;
    setState(() => _busy = false);
    await _growForReport();
    if (!mounted) return;
    await showTextReport(
      context,
      label,
      result.output.isEmpty ? '$label完成' : result.output,
    );
    await _close();
  }

  /// The flyout is a tiny menu-sized window, so a fixed-size report dialog would
  /// be clipped. Grow and recenter the window before showing the result so the
  /// full output is readable.
  Future<void> _growForReport() async {
    try {
      await windowManager.setSize(const Size(560, 420));
      // setAlignment recenters more reliably than center() for a frameless
      // always-on-top sub-window, so the report dialog is not pushed off-screen.
      await windowManager.setAlignment(Alignment.center);
    } catch (_) {}
  }

  /// Non-Codex install/recover targets are not supported yet; show a simple
  /// notice instead of running the CLI.
  Future<void> _showUnsupported() async {
    await showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('暂不支持', textAlign: TextAlign.center),
        content: const Text('该平台暂不支持，敬请期待。', textAlign: TextAlign.center),
        actionsAlignment: MainAxisAlignment.center,
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('确定'),
          ),
        ],
      ),
    );
  }

  Future<void> _tapAction(String label) async {
    if (_busy) return;
    UiLog.instance.info('flyout tap title=${widget.title} label=$label');
    switch (widget.title) {
      case '设置与模型':
        await _sendMain('show_main');
      case '高级':
        if (label == 'Fusion 设置...') {
          await _sendMain('show_fusion');
        } else {
          await _runAdvanced(label);
        }
      case '安装与恢复':
        final index = _installActions.indexWhere((item) => item.$1 == label);
        if (index < 0) return;
        final target = switch (index ~/ 2) {
          0 => 'Codex',
          1 => 'Claude Code',
          2 => 'DSH',
          3 => 'OpenCode',
          _ => 'Pi',
        };
        final isInstall = index.isEven;
        if (target != 'Codex') {
          await _showUnsupported();
          return;
        }
        if (target == 'Codex' && isInstall) {
          await _sendMain('show_install', {'target': 'codex'});
          return;
        }
        final command = switch (target) {
          'Codex' =>
            isInstall
                ? ['connect', 'codex', '--custom-only']
                : ['connect', 'remove', 'codex'],
          'Claude Code' =>
            isInstall ? ['connect', 'claude'] : ['connect', 'remove', 'claude'],
          'DSH' =>
            isInstall ? ['connect', 'dsh'] : ['connect', 'remove', 'dsh'],
          'OpenCode' =>
            isInstall
                ? ['connect', 'opencode']
                : ['connect', 'remove', 'opencode'],
          _ => isInstall ? ['connect', 'pi'] : ['connect', 'remove', 'pi'],
        };
        await _runClient(label, command, confirmRestore: !isInstall);
      case '关于':
        if (label == '关于 Codex Mixin...') {
          showAboutDialog(
            context: context,
            applicationName: 'Codex Mixin',
            applicationVersion: 'Windows 1.0.0',
            applicationIcon: const Icon(Icons.terminal_rounded, size: 52),
            children: const [Text('连接自定义模型供应商到 Codex 的本地网关。')],
          );
        } else if (label == '复制本地接口地址') {
          await widget.controller.copyEndpoint();
        } else if (label == '打开运行日志') {
          await widget.controller.openLogs();
        } else {
          await widget.controller.openConfigFolder();
        }
    }
  }

  List<(String, IconData)> get _actions => switch (widget.title) {
    '设置与模型' => const [
      ('供应商设置...', Icons.settings_outlined),
    ],
    '高级' => const [
      ('Fusion 设置...', Icons.account_tree_outlined),
      ('手动触发上报...', Icons.sync_outlined),
      ('导出明文配置...', Icons.file_upload_outlined),
    ],
    '安装与恢复' => _installActions,
    _ => const [
      ('关于 Codex Mixin...', Icons.info_outline),
      ('复制本地接口地址', Icons.link),
      ('打开运行日志', Icons.description_outlined),
      ('打开配置目录', Icons.folder_outlined),
    ],
  };

  @override
  Widget build(BuildContext context) {
    return Material(
      // Opaque background filling the whole window; the native window is
      // rounded by the Windows 11 compositor, so no transparent margin (which
      // would show black corners on the opaque native surface) is needed.
      color: Colors.white,
      child: Container(
        decoration: const BoxDecoration(color: Colors.white),
        child: ListView(
          shrinkWrap: true,
          padding: const EdgeInsets.symmetric(vertical: 6),
          children: [
            for (final action in _actions)
              InkWell(
                onTap: () => _tapAction(action.$1),
                child: Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 14,
                    vertical: 11,
                  ),
                  child: Row(
                    children: [
                      Icon(action.$2, size: 18, color: const Color(0xff3f4247)),
                      const SizedBox(width: 10),
                      Expanded(
                        child: Text(action.$1, overflow: TextOverflow.visible),
                      ),
                    ],
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

MaterialApp flyoutApp({
  required MixinController controller,
  required int windowId,
  required String title,
  required int trayWindowId,
}) {
  return MaterialApp(
    debugShowCheckedModeBanner: false,
    title: title,
    theme: mixinTheme(),
    locale: mixinLocale,
    supportedLocales: mixinSupportedLocales,
    localizationsDelegates: mixinLocalizationsDelegates,
    home: FlyoutPage(
      controller: controller,
      windowId: windowId,
      title: title,
      trayWindowId: trayWindowId,
    ),
  );
}
