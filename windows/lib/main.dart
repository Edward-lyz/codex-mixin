import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/material.dart';
import 'package:tray_manager/tray_manager.dart';
import 'package:window_manager/window_manager.dart';

import 'controller.dart';
import 'fusion_page.dart';
import 'flyout_page.dart';
import 'install_page.dart';
import 'log.dart';
import 'settings_page.dart';
import 'tray_page.dart';
import 'windows.dart';

export 'cli.dart' show decodeCliJson;
export 'flyout_page.dart' show FlyoutPage, flyoutApp;
export 'models.dart' show ProviderModel;
export 'provider_form.dart' show AddProviderDialog;
export 'settings_page.dart' show SettingsPage, settingsApp;
export 'tray_page.dart' show TrayPage, trayApp;

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  UiLog.instance.init();
  UiLog.instance.info('ui started; args=$args');
  runZonedGuarded(() async {
    if (args.firstOrNull == 'multi_window') {
      await _runSubWindow(args);
      return;
    }
    await _runMainWindow();
  }, (error, stack) => UiLog.instance.error('unhandled error', error, stack));
}

Future<void> _runMainWindow() async {
  await windowManager.ensureInitialized();
  await windowManager.setPreventClose(true);
  const windowOptions = WindowOptions(
    size: Size(1000, 650),
    minimumSize: Size(640, 500),
    center: true,
    backgroundColor: Colors.transparent,
    skipTaskbar: false,
    titleBarStyle: TitleBarStyle.hidden,
    windowButtonVisibility: false,
    title: '供应商设置',
  );
  await windowManager.waitUntilReadyToShow(windowOptions, () async {
    await windowManager.show();
    await windowManager.focus();
  });
  final controller = MixinController();
  if (Platform.isWindows) {
    // Honour the "start service at login" preference: the Run key only relaunches
    // the UI, so bring the gateway up here when the toggle is enabled.
    unawaited(() async {
      await controller.refreshLaunchAtLogin();
      if (controller.launchAtLogin) {
        await controller.startGatewayIfStopped();
      }
    }());
  }
  final hub = WindowHub.instance;
  hub.bindHandler();
  hub.onCommand = (method, arguments) async {
    UiLog.instance.info('main onCommand method=$method');
    switch (method) {
      case 'show_main':
        await hub.showMain();
      case 'show_tray':
        await hub.showTray();
      case 'show_fusion':
        await hub.showFusion();
      case 'show_install':
        await hub.showInstall('${arguments['target'] ?? 'codex'}');
      case 'show_flyout':
        await hub.showFlyout(arguments);
      case 'hide_flyout':
        await hub.hideFlyout();
      case 'refresh':
        await controller.refresh(force: true);
      case 'quit':
        await hub.closeAll();
        await windowManager.destroy();
    }
  };
  runApp(_MainShell(controller: controller, hub: hub));
}

Future<void> _runSubWindow(List<String> args) async {
  final windowId = int.tryParse(args.elementAtOrNull(1) ?? '') ?? 0;
  final argument = args.length > 2 ? args.sublist(2).join(' ') : '';
  final payload = parseWindowArgument(argument);
  final kind = '${payload['kind'] ?? windowKindTray}';
  final controller = MixinController();
  runApp(_subApp(kind, controller, windowId, payload));
  WidgetsBinding.instance.addPostFrameCallback((_) async {
    try {
      UiLog.instance.info('subwindow init kind=$kind id=$windowId');
      await windowManager.ensureInitialized();
      // waitUntilReadyToShow creates ITaskbarList3. setSkipTaskbar later
      // dereferences that pointer; calling it first is a native AV.
      await windowManager.waitUntilReadyToShow();
      if (kind == windowKindTray) {
        await windowManager.setAsFrameless();
        // Opaque background: the window is physically rounded by the Windows 11
        // compositor (see the desktop_multi_window rounding patch), so a
        // transparent background is unnecessary and only produced black corners
        // on the opaque native surface.
        await windowManager.setBackgroundColor(Colors.white);
        await windowManager.setAlwaysOnTop(true);
        await windowManager.setSkipTaskbar(true);
      } else if (kind == windowKindFlyout) {
        await windowManager.setAsFrameless();
        await windowManager.setBackgroundColor(Colors.white);
        await windowManager.setAlwaysOnTop(true);
        await windowManager.setSkipTaskbar(true);
      } else {
        await windowManager.setTitleBarStyle(
          TitleBarStyle.hidden,
          windowButtonVisibility: false,
        );
        await windowManager.setSkipTaskbar(false);
        final subSize = switch (kind) {
          windowKindInstall => const Size(560, 640),
          _ => const Size(620, 650),
        };
        await windowManager.setSize(subSize);
        // Keep sub-windows within a compact, readable range.
        await windowManager.setMinimumSize(switch (kind) {
          windowKindInstall => const Size(480, 540),
          _ => const Size(560, 520),
        });
        await windowManager.setMaximumSize(const Size(1500, 1000));
        // Titled sub-windows (Fusion / install) are separate
        // desktop_multi_window top-level windows that otherwise show a blank
        // default taskbar icon; set the bundled app icon explicitly.
        try {
          await windowManager.setIcon('assets/tray_icon.ico');
        } catch (_) {}
      }
      await windowManager.show();
      UiLog.instance.info('subwindow shown kind=$kind');
    } catch (e, s) {
      UiLog.instance.error('subwindow init failed kind=$kind', e, s);
    }
  });
  DesktopMultiWindow.setMethodHandler((call, fromWindowId) async {
    if (call.method == 'focus') {
      await windowManager.show();
      await windowManager.focus();
    } else if (call.method == 'configure') {
      final raw = call.arguments is String
          ? jsonDecode(call.arguments as String)
          : call.arguments;
      if (raw is Map && raw['alwaysOnTop'] == true) {
        await windowManager.setAlwaysOnTop(true);
      }
      if (raw is Map && raw['skipTaskbar'] == true) {
        await windowManager.waitUntilReadyToShow();
        await windowManager.setSkipTaskbar(true);
      }
    }
    return null;
  });
}

Widget _subApp(
  String kind,
  MixinController controller,
  int windowId,
  Map<String, dynamic> payload,
) {
  switch (kind) {
    case windowKindFusion:
      return fusionApp(controller: controller);
    case windowKindInstall:
      return installApp(
        controller: controller,
        target: '${payload['target'] ?? 'codex'}',
      );
    case windowKindFlyout:
      return flyoutApp(
        controller: controller,
        windowId: windowId,
        title: '${payload['title'] ?? ''}',
        trayWindowId: (payload['tray_window_id'] as num?)?.toInt() ?? 0,
      );
    default:
      return trayApp(controller: controller, windowId: windowId);
  }
}

class _MainShell extends StatefulWidget {
  final MixinController controller;
  final WindowHub hub;

  const _MainShell({required this.controller, required this.hub});

  @override
  State<_MainShell> createState() => _MainShellState();
}

class _MainShellState extends State<_MainShell>
    with TrayListener, WindowListener {
  bool _trayInitialized = false;

  @override
  void initState() {
    super.initState();
    windowManager.addListener(this);
    if (Platform.isWindows) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _initTray());
    }
  }

  @override
  void dispose() {
    if (_trayInitialized) trayManager.removeListener(this);
    windowManager.removeListener(this);
    super.dispose();
  }

  Future<void> _initTray() async {
    if (_trayInitialized) return;
    final executableDir = File(Platform.resolvedExecutable).parent.path;
    final candidates = [
      '${Directory.current.path}${Platform.pathSeparator}assets${Platform.pathSeparator}tray_icon.ico',
      '$executableDir${Platform.pathSeparator}data${Platform.pathSeparator}flutter_assets${Platform.pathSeparator}assets${Platform.pathSeparator}tray_icon.ico',
    ];
    final iconPath = candidates.firstWhere(
      (path) => File(path).existsSync(),
      orElse: () => candidates.first,
    );
    await trayManager.setIcon(iconPath);
    await trayManager.setToolTip('Codex Mixin');
    trayManager.addListener(this);
    _trayInitialized = true;
  }

  Future<void> _showTrayPopup() async {
    try {
      UiLog.instance.info('tray click -> showTray');
      await widget.hub.showTray();
      UiLog.instance.info('tray click -> showTray done');
    } catch (e, s) {
      UiLog.instance.error('showTrayPopup failed', e, s);
    }
  }

  @override
  void onTrayIconMouseDown() {
    UiLog.instance.info('onTrayIconMouseDown');
    unawaited(_showTrayPopup());
  }

  @override
  void onTrayIconRightMouseDown() {
    UiLog.instance.info('onTrayIconRightMouseDown');
    unawaited(_showTrayPopup());
  }

  @override
  void onWindowClose() {
    windowManager.hide();
  }

  @override
  Widget build(BuildContext context) {
    return settingsApp(controller: widget.controller);
  }
}

class CodexMixinApp extends StatelessWidget {
  final bool autoRefresh;
  final bool initialTrayPopup;

  const CodexMixinApp({
    super.key,
    this.autoRefresh = true,
    this.initialTrayPopup = false,
  });

  @override
  Widget build(BuildContext context) {
    final controller = MixinController();
    if (initialTrayPopup) {
      return trayApp(
        controller: controller,
        windowId: 0,
        autoRefresh: autoRefresh,
      );
    }
    return settingsApp(
      controller: controller,
      autoRefresh: autoRefresh,
      closeHides: false,
    );
  }
}
