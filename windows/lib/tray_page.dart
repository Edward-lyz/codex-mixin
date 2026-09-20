import 'dart:async';
import 'dart:convert';

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'client_integrations.dart';
import 'config_backups.dart';
import 'controller.dart';
import 'log.dart';
import 'models.dart';
import 'theme.dart';
import 'widgets.dart';

class TrayPage extends StatefulWidget {
  final MixinController controller;
  final int windowId;
  final bool autoRefresh;

  const TrayPage({
    super.key,
    required this.controller,
    required this.windowId,
    this.autoRefresh = true,
  });

  @override
  State<TrayPage> createState() => _TrayPageState();
}

class _TrayPageState extends State<TrayPage> with WindowListener {
  String? _selectedId;
  String? _selectedModelId;
  Timer? _refreshTimer;
  DateTime _ignoreBlurUntil = DateTime.fromMillisecondsSinceEpoch(0);
  // Submenus and their dialogs now render inside this tray window (instead of
  // separate frameless sub-windows) so clicks always register and dialogs are
  // positioned within the popup. `_submenuTitle`/`_submenuActions` drive the
  // in-window submenu; `_dialogDepth` keeps click-away dismissal from closing
  // the tray while a dialog is open.
  String? _submenuTitle;
  List<_MenuAction> _submenuActions = const [];
  int _dialogDepth = 0;
  bool _togglingGateway = false;

  MixinController get _controller => widget.controller;
  GatewaySnapshot get _snapshot => _controller.snapshot;
  ProviderModel? get _selected => _snapshot.providers
      .where((provider) => provider.id == _selectedId)
      .firstOrNull;

  @override
  void initState() {
    super.initState();
    windowManager.addListener(this);
    _selectedId = _snapshot.providers.firstOrNull?.id;
    if (widget.autoRefresh) {
      unawaited(_controller.refreshLaunchAtLogin());
      WidgetsBinding.instance.addPostFrameCallback((_) => _refresh());
      _refreshTimer = Timer.periodic(
        const Duration(seconds: 10),
        (_) => _refresh(),
      );
    }
  }

  @override
  void dispose() {
    windowManager.removeListener(this);
    _refreshTimer?.cancel();
    super.dispose();
  }

  @override
  void onWindowBlur() {
    if (DateTime.now().isBefore(_ignoreBlurUntil)) return;
    // Keep the tray open while an in-window dialog is showing; the dialog holds
    // focus, so a blur here is spurious.
    if (_dialogDepth > 0) return;
    unawaited(_hide());
  }

  @override
  void onWindowFocus() {}

  Future<void> _refresh() async {
    await _controller.refresh();
    if (!mounted) return;
    setState(() {
      if (_selectedId == null ||
          !_snapshot.providers.any((provider) => provider.id == _selectedId)) {
        _selectedId = _snapshot.providers.firstOrNull?.id;
      }
    });
  }

  Future<void> _hide() async {
    // Return to the dashboard so the next open starts at the top-level menu.
    if (mounted) {
      setState(() => _submenuTitle = null);
    }
    await windowManager.hide();
  }

  Future<void> _notifyMain(String method, [Map<String, dynamic>? args]) async {
    _ignoreBlurUntil = DateTime.now().add(const Duration(seconds: 2));
    await DesktopMultiWindow.invokeMethod(
      0,
      'command',
      jsonEncode({'method': method, ...?args}),
    );
  }

  /// Open a submenu inline within the tray popup.
  void _openSubmenu(String title, List<_MenuAction> actions) {
    setState(() {
      _submenuTitle = title;
      _submenuActions = actions;
    });
  }

  void _closeSubmenu() {
    setState(() => _submenuTitle = null);
  }

  /// Ask the main window to open one of the standalone windows (供应商设置 /
  /// Fusion 设置), then dismiss the tray popup.
  Future<void> _openWindow(String method) async {
    UiLog.instance.info('tray open window method=$method');
    await _notifyMain(method);
    await _hide();
  }

  /// Show a dialog while keeping the tray popup open (dialogs render in this
  /// window, so click-away dismissal must be suspended for their lifetime).
  Future<T?> _withDialog<T>(Future<T?> Function() show) async {
    _dialogDepth++;
    _ignoreBlurUntil = DateTime.now().add(const Duration(seconds: 8));
    try {
      return await show();
    } finally {
      _dialogDepth--;
    }
  }

  /// Open the reusable install-config window for Codex (runs the install
  /// in-window with full space), then dismiss the tray popup.
  Future<void> _installCodex() async {
    await _notifyMain('show_install', {'target': 'codex'});
    await _hide();
  }

  Future<void> _runClient(
    String title,
    List<String> args, {
    bool confirmRestore = false,
    String? confirmMessage,
  }) async {
    if (confirmRestore) {
      final confirmed = await _withDialog(
        () => confirmAction(
          context,
          title: title,
          message: confirmMessage ?? '确定执行该操作？',
        ),
      );
      if (confirmed != true) return;
    }
    _ignoreBlurUntil = DateTime.now().add(const Duration(seconds: 8));
    await _withDialog(
      () => runWithProgress(
        context,
        title: title,
        resultText: (result) => result.ok
            ? '$title成功'
            : '$title失败：${formatCliReport(result.output).trim().isEmpty ? '未返回具体原因' : formatCliReport(result.output).trim()}',
        action: (onProgress) =>
            _controller.runAction(title, args, onProgress: onProgress),
      ),
    );
    if (!mounted) return;
    setState(() {});
    await _refresh();
  }

  Future<void> _exportConfig() async {
    final path = await chooseBackupExportPath();
    if (path == null || !mounted) return;
    await _withDialog(
      () => runWithProgress(
        context,
        title: '导出配置备份',
        action: (_) => _controller.exportConfig(path),
      ),
    );
  }

  Future<void> _importConfig() async {
    final path = await chooseBackupImportPath();
    if (path == null || !mounted) return;
    final confirmed = await _withDialog(
      () => confirmAction(
        context,
        title: '导入配置备份？',
        message: '这会替换 Provider、模型、Fusion 和凭据配置，然后重启本地网关。',
      ),
    );
    if (confirmed != true || !mounted) return;
    await _withDialog(
      () => runWithProgress(
        context,
        title: '导入配置备份',
        action: (_) => _controller.importConfig(path),
      ),
    );
    await _refresh();
  }

  @override
  Widget build(BuildContext context) {
    return Material(
      color: Colors.white,
      child: Container(
        decoration: const BoxDecoration(color: Colors.white),
        // The dashboard fills the fixed window height (see _dashboardView); the
        // submenu can be taller than the window, so it scrolls.
        child: _submenuTitle != null
            ? SingleChildScrollView(child: _submenuView())
            : _dashboardView(),
      ),
    );
  }

  Widget _submenuView() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(8, 8, 8, 10),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              IconButton(
                icon: const Icon(Icons.arrow_back, size: 20),
                onPressed: _closeSubmenu,
                tooltip: '返回',
              ),
              const SizedBox(width: 4),
              Expanded(
                child: Text(
                  _submenuTitle ?? '',
                  style: const TextStyle(
                    fontSize: 16,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
            ],
          ),
          const Divider(height: 8),
          Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final action in _submenuActions)
                HoverButton(
                  onTap: action.onTap,
                  borderRadius: 9,
                  padding: const EdgeInsets.symmetric(
                    horizontal: 8,
                    vertical: 12,
                  ),
                  child: Row(
                    children: [
                      Icon(
                        action.icon,
                        size: 20,
                        color: const Color(0xff3f4247),
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Text(
                          action.label,
                          style: const TextStyle(fontSize: 14),
                        ),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _dashboardView() {
    final provider = _selected;
    return SizedBox.expand(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(18, 12, 18, 10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _serviceCard(),
            const SizedBox(height: 12),
            _providerTabs(),
            const Divider(height: 14),
            // Fixed-height token/provider region. It flexes to fill the space
            // between the top block and the bottom menu, so the popup's overall
            // height never changes. Content is top-aligned; whether or not
            // there is token data the area keeps the same size and the menu
            // below stays pinned to the bottom.
            Expanded(child: _providerRegion(provider)),
            const Divider(height: 12),
            _menuItems(),
          ],
        ),
      ),
    );
  }

  Widget _providerRegion(ProviderModel? provider) {
    if (provider == null) {
      return const Align(
        alignment: Alignment.topCenter,
        child: Padding(
          padding: EdgeInsets.only(top: 24),
          child: Text('还没有供应商', style: TextStyle(color: muted)),
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            providerLogo(provider, 32),
            const SizedBox(width: 9),
            Expanded(
              child: Text(
                provider.displayName,
                style: const TextStyle(
                  fontSize: 15,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ],
        ),
        const SizedBox(height: 10),
        _quota(provider),
        const SizedBox(height: 10),
        // Token area flexes to fill the rest of the fixed-height region so the
        // empty-state message can center vertically instead of leaving a large
        // gap below it.
        Expanded(child: _tokenUsage(provider)),
      ],
    );
  }

  Widget _serviceCard() => Row(
    children: [
      Container(
        width: 10,
        height: 10,
        decoration: BoxDecoration(
          color: _snapshot.gatewayRunning ? green : Colors.grey,
          shape: BoxShape.circle,
        ),
      ),
      const SizedBox(width: 10),
      Expanded(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              _snapshot.serviceTitle,
              style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 1),
            Text(
              _snapshot.gatewayRunning ? _snapshot.serviceEndpoint : '网关当前未运行',
              style: const TextStyle(color: muted, fontSize: 11),
            ),
          ],
        ),
      ),
      Transform.scale(
        scale: 0.78,
        alignment: Alignment.centerRight,
        child: Switch(
          materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
          value: _snapshot.gatewayRunning,
          onChanged: _controller.busy || _togglingGateway
              ? null
              : (_) async {
                  setState(() => _togglingGateway = true);
                  try {
                    await _controller.toggleGateway();
                  } finally {
                    if (mounted) setState(() => _togglingGateway = false);
                  }
                },
        ),
      ),
    ],
  );

  Widget _providerTabs() => SizedBox(
    height: 44,
    child: ListView.separated(
      scrollDirection: Axis.horizontal,
      itemCount: _snapshot.providers.length,
      separatorBuilder: (_, _) => const SizedBox(width: 7),
      itemBuilder: (_, index) {
        final provider = _snapshot.providers[index];
        final selected = provider.id == _selectedId;
        return HoverButton(
          width: 46,
          borderRadius: 10,
          baseColor: selected ? const Color(0xffe4ebff) : Colors.transparent,
          borderColor: selected ? const Color(0xffb7c8ff) : Colors.transparent,
          padding: const EdgeInsets.all(6),
          onTap: () => setState(() {
            _selectedId = provider.id;
            _selectedModelId = null;
          }),
          child: providerLogo(provider, 34),
        );
      },
    ),
  );

  Widget _quota(ProviderModel provider) {
    final rows = _snapshot.quotaRows
        .where((row) => row['provider_id'] == provider.id)
        .toList();
    if (rows.isEmpty) {
      return const Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [
          Text('额度', style: TextStyle(fontWeight: FontWeight.w700)),
          Text('暂无数据', style: TextStyle(color: muted)),
        ],
      );
    }
    return Column(
      children: rows.take(3).map((row) {
        final remaining = row['remaining'];
        final used = row['used'];
        final limit = row['limit'];
        final currency = '${row['currency'] ?? ''}'.trim();
        final suffix = currency.isEmpty ? '' : ' $currency';
        final ratio = used is num && limit is num && limit > 0
            ? (used / limit).clamp(0, 1).toDouble()
            : null;
        // Prefer "已用 / 总额度" when both are known; otherwise fall back to the
        // remaining balance the provider reports.
        final String valueText;
        if (used is num && limit is num) {
          valueText =
              '已用 ${used.toStringAsFixed(2)} / 总 ${limit.toStringAsFixed(2)}$suffix';
        } else if (remaining is num) {
          valueText = '余额 ${remaining.toStringAsFixed(2)}$suffix';
        } else {
          valueText = '${remaining ?? used ?? row['value'] ?? ''}$suffix';
        }
        return Padding(
          padding: const EdgeInsets.only(bottom: 7),
          child: Column(
            children: [
              Row(
                mainAxisAlignment: MainAxisAlignment.spaceBetween,
                children: [
                  const Text(
                    '额度',
                    style: TextStyle(fontWeight: FontWeight.w700),
                  ),
                  Text(valueText),
                ],
              ),
              if (ratio != null) ...[
                const SizedBox(height: 4),
                ClipRRect(
                  borderRadius: BorderRadius.circular(99),
                  child: LinearProgressIndicator(
                    value: ratio,
                    minHeight: 4,
                    backgroundColor: const Color(0xffeceef2),
                  ),
                ),
              ],
            ],
          ),
        );
      }).toList(),
    );
  }

  Widget _tokenUsage(ProviderModel provider) {
    final rows = _snapshot.usageRows
        .where((row) => row['provider_id'] == provider.id)
        .toList();
    rows.sort((a, b) {
      final delta = _usageTotal(b).compareTo(_usageTotal(a));
      if (delta != 0) return delta;
      return '${a['model_id']}'.compareTo('${b['model_id']}');
    });
    final selected = rows.where((row) => row['model_id'] == _selectedModelId);
    final selectedRow = selected.isEmpty ? null : selected.first;
    final maxTokens = rows.fold<num>(0, (sum, row) {
      final total = _usageTotal(row);
      return total > sum ? total : sum;
    });
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Center(
          child: Container(
            padding: const EdgeInsets.all(3),
            decoration: BoxDecoration(
              color: const Color(0xffeef0f3),
              borderRadius: BorderRadius.circular(20),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (final option in <({String label, int? days})>[
                  (label: '1天', days: 1),
                  (label: '7天', days: 7),
                  (label: '1月', days: 30),
                  (label: '全部', days: null),
                ])
                  _rangeChip(option.label, option.days),
              ],
            ),
          ),
        ),
        const SizedBox(height: 8),
        Expanded(
          child: rows.isEmpty
              ? Center(
                  child: Text(
                    !_snapshot.gatewayRunning
                        ? '网关未运行，无法读取 Token 使用'
                        : _snapshot.usageError != null
                        ? 'Token 使用暂不可用：${_snapshot.usageError}'
                        : '当前供应商暂无 Token 使用记录\n'
                              '测速结果不计入此面板，请通过 Codex 请求后刷新',
                    textAlign: TextAlign.center,
                    style: const TextStyle(
                      color: muted,
                      fontSize: 12,
                      height: 1.5,
                    ),
                  ),
                )
              : LayoutBuilder(
                  builder: (context, region) {
                    // Bars fill the region so there is no empty gap below the
                    // chart. Keep a minimum so the value/name labels never
                    // overflow in a very short popup; scroll if even that plus
                    // the optional detail card cannot fit.
                    const minBars = 96.0;
                    const detailBlock = 118.0;
                    final hasDetail = selectedRow != null;
                    var barsHeight =
                        region.maxHeight - (hasDetail ? detailBlock : 0);
                    if (barsHeight < minBars) barsHeight = minBars;
                    final body = Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        SizedBox(
                          height: barsHeight,
                          child: LayoutBuilder(
                            builder: (context, constraints) {
                              final barMax = (constraints.maxHeight - 36).clamp(
                                8.0,
                                600.0,
                              );
                              return ListView.separated(
                                scrollDirection: Axis.horizontal,
                                itemCount: rows.length,
                                separatorBuilder: (_, _) =>
                                    const SizedBox(width: 6),
                                itemBuilder: (context, index) {
                                  final row = rows[index];
                                  final id = '${row['model_id'] ?? ''}';
                                  final total = _usageTotal(row);
                                  final selectedBar = id == _selectedModelId;
                                  final height = maxTokens <= 0
                                      ? 3.0
                                      : (barMax * (total / maxTokens)).clamp(
                                          3.0,
                                          barMax,
                                        );
                                  return GestureDetector(
                                    key: ValueKey('token-bar-$id'),
                                    onTap: () => setState(() {
                                      _selectedModelId = selectedBar
                                          ? null
                                          : id;
                                    }),
                                    child: SizedBox(
                                      width: 47,
                                      child: Column(
                                        children: [
                                          Text(
                                            formatTokenCount(total),
                                            style: TextStyle(
                                              fontSize: 10,
                                              fontWeight: selectedBar
                                                  ? FontWeight.w700
                                                  : FontWeight.w500,
                                              color: selectedBar
                                                  ? const Color(0xff1d2024)
                                                  : muted,
                                            ),
                                          ),
                                          const SizedBox(height: 4),
                                          Expanded(
                                            child: Align(
                                              alignment: Alignment.bottomCenter,
                                              child: AnimatedContainer(
                                                duration: const Duration(
                                                  milliseconds: 180,
                                                ),
                                                width: selectedBar ? 9 : 7,
                                                height: height,
                                                decoration: BoxDecoration(
                                                  color: accent.withValues(
                                                    alpha: selectedBar
                                                        ? 1
                                                        : 0.78,
                                                  ),
                                                  borderRadius:
                                                      BorderRadius.circular(99),
                                                ),
                                              ),
                                            ),
                                          ),
                                          const SizedBox(height: 4),
                                          Tooltip(
                                            message: id,
                                            waitDuration: const Duration(
                                              milliseconds: 300,
                                            ),
                                            child: Text(
                                              id,
                                              maxLines: 1,
                                              overflow: TextOverflow.ellipsis,
                                              style: TextStyle(
                                                fontSize: 10,
                                                fontWeight: selectedBar
                                                    ? FontWeight.w700
                                                    : FontWeight.w400,
                                                color: selectedBar
                                                    ? accent
                                                    : muted,
                                              ),
                                            ),
                                          ),
                                        ],
                                      ),
                                    ),
                                  );
                                },
                              );
                            },
                          ),
                        ),
                        if (selectedRow != null) ...[
                          const SizedBox(height: 10),
                          _tokenDetail(selectedRow),
                        ],
                      ],
                    );
                    return (barsHeight + (hasDetail ? detailBlock : 0)) <=
                            region.maxHeight
                        ? body
                        : SingleChildScrollView(child: body);
                  },
                ),
        ),
      ],
    );
  }

  Widget _rangeChip(String label, int? days) {
    final selected = _controller.usageDays == days;
    return HoverButton(
      borderRadius: 16,
      baseColor: selected ? Colors.white : Colors.transparent,
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
      onTap: () async {
        if (_controller.usageDays == days) return;
        setState(() {
          _controller.usageDays = days;
          _selectedModelId = null;
        });
        await _refresh();
      },
      child: Text(
        label,
        style: TextStyle(
          fontSize: 12,
          fontWeight: FontWeight.w600,
          color: selected ? const Color(0xff1d2024) : muted,
        ),
      ),
    );
  }

  Widget _tokenDetail(Map<String, dynamic> row) {
    Widget metric(String label, String value) => Expanded(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label, style: const TextStyle(color: muted, fontSize: 11)),
          const SizedBox(height: 2),
          Text(
            value,
            style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w700),
          ),
        ],
      ),
    );
    final cacheHit = row['cache_hit_percent'];
    final ttft = row['average_ttft_ms'];
    final tps = row['output_tps'];
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
      decoration: BoxDecoration(
        color: const Color(0xfff6f7f9),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '${row['model_id'] ?? ''}',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontWeight: FontWeight.w700, fontSize: 12),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              metric('请求', '${row['request_count'] ?? 0}'),
              metric('输入', formatTokenCount(row['input_tokens'])),
              metric('缓存输入', formatTokenCount(row['cache_read_tokens'])),
              metric('输出', formatTokenCount(row['output_tokens'])),
            ],
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              metric(
                '缓存比例',
                cacheHit is num ? '${cacheHit.toStringAsFixed(1)}%' : '未上报',
              ),
              metric('缓存输出', formatTokenCount(row['cache_creation_tokens'])),
              metric('首字响应', ttft is num ? '${ttft.round()} ms' : '未上报'),
              metric(
                '每秒吞吐',
                tps is num ? '${tps.toStringAsFixed(1)} tok/s' : '未上报',
              ),
            ],
          ),
        ],
      ),
    );
  }

  num _usageTotal(Map<String, dynamic> row) {
    num read(String key) {
      final value = row[key];
      return value is num ? value : 0;
    }

    return read('input_tokens') +
        read('cache_read_tokens') +
        read('cache_creation_tokens') +
        read('output_tokens');
  }

  Widget _menuItems() => Column(
    children: [
      _item(Icons.dns_outlined, '模型与服务...', () => _openWindow('show_main')),
      _item(
        _controller.launchAtLogin ? Icons.check : Icons.login,
        '登录时启动并开启服务',
        () async {
          await _controller.toggleLaunchAtLogin();
          if (mounted) setState(() {});
        },
      ),
      _item(Icons.health_and_safety_outlined, '健康检测和修复', () async {
        final result = await _controller.doctor();
        if (!mounted) return;
        await _withDialog(
          () => showTextReport(context, '健康检测和修复', result.output),
        );
        await _refresh();
      }),
      const Divider(height: 12),
      _item(Icons.settings_suggest_outlined, '高级', () {
        _openSubmenu('高级', [
          _MenuAction(
            Icons.account_tree_outlined,
            'Fusion 设置',
            () => _openWindow('show_fusion'),
          ),
          _MenuAction(Icons.sync_outlined, '手动触发上报', () async {
            await _withDialog(
              () => runWithProgress(
                context,
                title: '手动触发上报',
                resultText: (result) {
                  final detail = formatCliReport(result.output).trim();
                  if (result.ok) {
                    return detail.isEmpty ? '上报已触发' : detail;
                  }
                  return detail.isEmpty ? '上报失败' : '上报失败：$detail';
                },
                action: (_) => _controller.reportReplay(),
              ),
            );
          }),
          _MenuAction(
            Icons.file_download_outlined,
            '导入配置备份',
            _importConfig,
          ),
          _MenuAction(
            Icons.file_upload_outlined,
            '导出配置备份',
            _exportConfig,
          ),
        ]);
      }, chevron: true),
      _item(Icons.file_download_outlined, '安装与恢复', () {
        _openSubmenu('安装与恢复', [
          for (final client in clientIntegrations) ...[
            _MenuAction(
              Icons.file_download_outlined,
              client.installLabel,
              client.id == 'codex'
                  ? _installCodex
                  : () => _runClient(
                      client.installLabel,
                      client.installArguments,
                    ),
            ),
            _MenuAction(
              Icons.restore,
              client.removeLabel,
              () => _runClient(
                client.removeLabel,
                client.removeArguments,
                confirmRestore: true,
                confirmMessage: '会恢复安装前备份的 ${client.displayName} 配置。',
              ),
            ),
          ],
        ]);
      }, chevron: true),
      _item(Icons.info_outline, '关于', () {
        _openSubmenu('关于', [
          _MenuAction(Icons.info_outline, '关于 Codex Mixin', () {
            _withDialog(() async {
              showAboutDialog(
                context: context,
                applicationName: 'Codex Mixin',
                applicationVersion: 'Windows $mixinVersion',
                applicationIcon: const Icon(Icons.terminal_rounded, size: 52),
                children: const [Text('连接自定义模型供应商到 Codex 的本地网关。')],
              );
              return null;
            });
          }),
          _MenuAction(
            Icons.system_update_alt,
            '检查更新',
            _controller.openReleasePage,
          ),
          _MenuAction(Icons.link, '复制本地接口地址', _controller.copyEndpoint),
          _MenuAction(
            Icons.description_outlined,
            '打开运行日志',
            _controller.openLogs,
          ),
          _MenuAction(
            Icons.folder_outlined,
            '打开配置目录',
            _controller.openConfigFolder,
          ),
        ]);
      }, chevron: true),
      const Divider(height: 12),
      _item(
        Icons.power_settings_new,
        '退出 Codex Mixin',
        () => _notifyMain('quit'),
      ),
    ],
  );

  Widget _item(
    IconData icon,
    String label,
    VoidCallback action, {
    bool chevron = false,
  }) {
    return HoverButton(
      onTap: action,
      borderRadius: 9,
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
      child: Row(
        children: [
          Icon(icon, size: 21, color: const Color(0xff3f4247)),
          const SizedBox(width: 14),
          Expanded(child: Text(label, style: const TextStyle(fontSize: 15))),
          if (chevron) const Icon(Icons.chevron_right, size: 22),
        ],
      ),
    );
  }
}

class _MenuAction {
  final IconData icon;
  final String label;
  final VoidCallback onTap;
  const _MenuAction(this.icon, this.label, this.onTap);
}

MaterialApp trayApp({
  required MixinController controller,
  required int windowId,
  bool autoRefresh = true,
}) {
  return MaterialApp(
    debugShowCheckedModeBanner: false,
    title: 'Codex Mixin',
    theme: mixinTheme(),
    locale: mixinLocale,
    supportedLocales: mixinSupportedLocales,
    localizationsDelegates: mixinLocalizationsDelegates,
    color: Colors.transparent,
    home: TrayPage(
      controller: controller,
      windowId: windowId,
      autoRefresh: autoRefresh,
    ),
  );
}
