import 'dart:convert';
import 'dart:math' as math;
import 'dart:ui';

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:screen_retriever/screen_retriever.dart';
import 'package:tray_manager/tray_manager.dart';
import 'package:window_manager/window_manager.dart';

import 'log.dart';

const windowKindMain = 'main';
const windowKindTray = 'tray';
const windowKindFusion = 'fusion';
const windowKindFlyout = 'flyout';
const windowKindInstall = 'install';

class WindowHub {
  WindowHub._();
  static final WindowHub instance = WindowHub._();

  final Map<String, int> _ids = {};
  Future<void> Function(String method, Map<String, dynamic> args)? onCommand;

  Future<void> showMain() async {
    await windowManager.setSkipTaskbar(false);
    await windowManager.show();
    await windowManager.focus();
  }

  Future<void> hideMain() async {
    await windowManager.hide();
  }

  Future<void>? _showing;

  Future<void> showTray({Offset? position}) {
    return _serialize(() async {
      final placement = await trayPopupPlacement();
      await _show(
        windowKindTray,
        'Codex Mixin',
        placement.size,
        position: position ?? placement.position,
      );
    });
  }

  Future<void> hideTray() => _hide(windowKindTray);

  Future<void> showFlyout(Map<String, dynamic> args) async {
    // Only one submenu is visible at a time; drop any previous flyout first.
    await hideFlyout();
    final title = '${args['flyout_title'] ?? ''}';
    final trayWindowId = (args['tray_window_id'] as num?)?.toInt() ?? 0;
    final count = (args['action_count'] as num?)?.toInt() ?? 1;
    final dpr = (args['dpr'] as num?)?.toDouble() ?? 1.0;
    final trayLeft = (args['tray_left'] as num?)?.toDouble() ?? 0;
    final trayTop = (args['tray_top'] as num?)?.toDouble() ?? 0;
    final trayWidth = (args['tray_width'] as num?)?.toDouble() ?? 0;
    final trayHeight = (args['tray_height'] as num?)?.toDouble() ?? 0;
    final anchorTop = (args['anchor_top'] as num?)?.toDouble() ?? 0;

    // Resolve the display that hosts the tray (screen_retriever is only
    // registered in this main isolate) and clamp the submenu to its work area
    // both horizontally (left/right of the tray) and vertically.
    Rect workAreaFor(Display display) {
      final origin = display.visiblePosition ?? Offset.zero;
      final extent = display.visibleSize ?? display.size;
      return Rect.fromLTWH(origin.dx, origin.dy, extent.width, extent.height);
    }

    final displays = await screenRetriever.getAllDisplays();
    final primary = await screenRetriever.getPrimaryDisplay();
    final trayCenter = Offset(
      trayLeft + trayWidth / 2,
      trayTop + trayHeight / 2,
    );
    final display = displays.isEmpty
        ? primary
        : displays.reduce(
            (best, candidate) =>
                workAreaFor(candidate).contains(trayCenter) ? candidate : best,
          );
    final workArea = workAreaFor(display);
    final scale = (display.scaleFactor ?? dpr).toDouble();

    const menuWidth = 300.0;
    final menuHeight = (count * 44.0 + 12.0).clamp(120.0, 520.0);
    const gap = 8.0;

    var left = trayLeft - menuWidth - gap;
    if (left < workArea.left) left = trayLeft + trayWidth + gap;
    left = left.clamp(
      workArea.left,
      math.max(workArea.left, workArea.right - menuWidth),
    );
    var top = trayTop + anchorTop;
    if (top + menuHeight > workArea.bottom) {
      top = workArea.bottom - menuHeight;
    }
    top = top.clamp(workArea.top, math.max(workArea.top, workArea.bottom));

    UiLog.instance.info(
      'showFlyout title=$title tray=($trayLeft,$trayTop ${trayWidth}x$trayHeight) '
      'workArea=$workArea scale=$scale -> ($left,$top ${menuWidth}x$menuHeight)',
    );
    await _show(
      '$windowKindFlyout:$title',
      title,
      Size(menuWidth * scale, menuHeight * scale),
      position: Offset(left * scale, top * scale),
      payload: {
        'kind': windowKindFlyout,
        'title': title,
        'tray_window_id': trayWindowId,
      },
    );
  }

  Future<void> hideFlyout() async {
    final keys = _ids.keys
        .where((key) => key.startsWith('$windowKindFlyout:'))
        .toList();
    for (final key in keys) {
      final id = _ids.remove(key);
      if (id == null) continue;
      // Close (not just hide) so each open rebuilds fresh window state; this
      // avoids stale focus/dismiss guards when a submenu is reopened.
      try {
        await WindowController.fromWindowId(id).close();
      } catch (_) {}
    }
  }

  Future<void> showFusion() =>
      _show(windowKindFusion, 'Fusion 设置', const Size(620, 650));

  /// Open the reusable install-config window for a downstream platform. Keyed by
  /// target so each platform reuses its own window instance; the page renders
  /// from the target descriptor (see install_page.dart).
  Future<void> showInstall(String target) => _show(
    '$windowKindInstall:$target',
    '安装配置',
    const Size(560, 640),
    payload: {'kind': windowKindInstall, 'target': target},
  );

  // The tray popup uses a fixed logical size. The dashboard fills this height:
  // the token/provider region flexes to absorb any content variation, so the
  // overall popup height never changes and the bottom menu stays pinned.
  static const trayPopupLogicalSize = Size(340, 700);

  Future<({Offset position, Size size})> trayPopupPlacement() async {
    const edge = 8.0;
    final displays = await screenRetriever.getAllDisplays();
    final primary = await screenRetriever.getPrimaryDisplay();
    Rect? icon;
    try {
      icon = await trayManager.getBounds();
    } catch (_) {
      icon = null;
    }
    Rect logicalWorkAreaFor(Display display) {
      final position = display.visiblePosition ?? Offset.zero;
      final size = display.visibleSize ?? display.size;
      return Rect.fromLTWH(position.dx, position.dy, size.width, size.height);
    }

    double distanceToWorkArea(Rect area, Offset point) {
      final dx = point.dx < area.left
          ? area.left - point.dx
          : point.dx > area.right
          ? point.dx - area.right
          : 0.0;
      final dy = point.dy < area.top
          ? area.top - point.dy
          : point.dy > area.bottom
          ? point.dy - area.bottom
          : 0.0;
      return dx * dx + dy * dy;
    }

    final primaryWorkArea = logicalWorkAreaFor(primary);
    final trayIcon = icon;
    final display = trayIcon == null || displays.isEmpty
        ? primary
        : displays.reduce((best, candidate) {
            final point = trayIcon.center;
            return distanceToWorkArea(logicalWorkAreaFor(candidate), point) <
                    distanceToWorkArea(logicalWorkAreaFor(best), point)
                ? candidate
                : best;
          });
    final displayScale = (display.scaleFactor ?? 1).toDouble();
    final mainViewScale = PlatformDispatcher.instance.views.isEmpty
        ? 1.0
        : PlatformDispatcher.instance.views.first.devicePixelRatio;
    final size = Size(
      math.min(
        trayPopupLogicalSize.width * displayScale,
        math.max(1.0, logicalWorkAreaFor(display).width - edge * 2) *
            displayScale,
      ),
      math.min(
        (trayPopupLogicalSize.height) * displayScale,
        math.max(1.0, logicalWorkAreaFor(display).height - edge * 2) *
            displayScale,
      ),
    );
    final logicalWorkArea = logicalWorkAreaFor(display);
    final workArea = Rect.fromLTWH(
      logicalWorkArea.left * displayScale,
      logicalWorkArea.top * displayScale,
      logicalWorkArea.width * displayScale,
      logicalWorkArea.height * displayScale,
    );
    final edgePx = edge * displayScale;
    final validIcon =
        trayIcon != null &&
        trayIcon.width >= 8 &&
        trayIcon.height >= 8 &&
        trayIcon.left.isFinite &&
        trayIcon.top.isFinite;
    final anchorX = validIcon
        ? trayIcon.center.dx * mainViewScale
        : workArea.right - 44;
    final x = (anchorX - size.width / 2)
        .clamp(workArea.left + edgePx, workArea.right - size.width - edgePx)
        .toDouble();
    final y = (workArea.bottom - size.height - edgePx)
        .clamp(workArea.top + edgePx, workArea.bottom - edgePx)
        .toDouble();
    UiLog.instance.info(
      'trayPopupOrigin icon=$icon logicalWorkArea=$logicalWorkArea '
      'workArea=$workArea primary=$primaryWorkArea display=${display.name} '
      'scale=$displayScale '
      'size=${size.width.toStringAsFixed(0)}x${size.height.toStringAsFixed(0)} '
      '-> Offset(${x.toStringAsFixed(0)}, ${y.toStringAsFixed(0)})',
    );
    return (position: Offset(x, y), size: size);
  }

  Future<void> _serialize(Future<void> Function() action) {
    final previous = _showing;
    final current = () async {
      if (previous != null) {
        try {
          await previous;
        } catch (_) {}
      }
      await action();
    }();
    _showing = current;
    return current;
  }

  Future<void> _show(
    String kind,
    String title,
    Size size, {
    Offset? position,
    Map<String, dynamic>? payload,
  }) async {
    final existing = _ids[kind];
    if (existing != null) {
      final controller = WindowController.fromWindowId(existing);
      try {
        await controller.show();
        if (position != null) {
          await controller.setFrame(position & size);
        }
        await DesktopMultiWindow.invokeMethod(existing, 'focus', '{}');
        return;
      } catch (_) {
        _ids.remove(kind);
      }
    }
    try {
      UiLog.instance.info('createWindow kind=$kind');
      final window = await DesktopMultiWindow.createWindow(
        jsonEncode(payload ?? {'kind': kind, 'title': title}),
      );
      _ids[kind] = window.windowId;
      await window.setFrame((position ?? const Offset(80, 80)) & size);
      await window.setTitle(title);
      await window.show();
      UiLog.instance.info('createWindow kind=$kind done id=${window.windowId}');
    } catch (e, s) {
      UiLog.instance.error('createWindow kind=$kind failed', e, s);
    }
  }

  Future<void> _hide(String kind) async {
    final id = _ids[kind];
    if (id == null) return;
    try {
      await WindowController.fromWindowId(id).hide();
    } catch (_) {
      _ids.remove(kind);
    }
  }

  Future<void> closeAll() async {
    for (final id in _ids.values) {
      try {
        await WindowController.fromWindowId(id).close();
      } catch (_) {}
    }
    _ids.clear();
  }

  void bindHandler() {
    DesktopMultiWindow.setMethodHandler((call, fromWindowId) async {
      final args = _decode(call.arguments);
      switch (call.method) {
        case 'closed':
          _ids.removeWhere((_, value) => value == fromWindowId);
          return null;
        case 'command':
          final method = '${args['method'] ?? ''}';
          await onCommand?.call(method, args);
          return null;
        default:
          return null;
      }
    });
  }

  Map<String, dynamic> _decode(dynamic raw) {
    if (raw is Map<String, dynamic>) return raw;
    if (raw is String && raw.isNotEmpty) {
      final value = jsonDecode(raw);
      if (value is Map<String, dynamic>) return value;
    }
    return const {};
  }
}

Map<String, dynamic> parseWindowArgument(String argument) {
  if (argument.isEmpty) return const {};
  try {
    final value = jsonDecode(argument);
    if (value is Map<String, dynamic>) return value;
  } catch (_) {}
  return const {};
}
