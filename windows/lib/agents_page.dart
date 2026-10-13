import 'dart:async';

import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'agent_models.dart';
import 'cli.dart';
import 'controller.dart';
import 'theme.dart';
import 'widgets.dart';

/// Windows error red, used for detection and upgrade failures.
const _danger = Color(0xffd13438);

IconData _agentIcon(String id) => switch (id) {
  'codex' => Icons.terminal_rounded,
  'claude' => Icons.psychology_outlined,
  'claude-desktop' => Icons.desktop_windows_outlined,
  'dsh' => Icons.dns_outlined,
  'opencode' => Icons.code_rounded,
  'pi' => Icons.functions_rounded,
  _ => Icons.smart_toy_outlined,
};

/// Agent 接入管理：程序检测、接入状态、版本与升级，全部来自 `agent` CLI JSON。
///
/// The list paints as soon as `agent list --json` returns; versions arrive
/// later through `agent versions --json --refresh` so a slow npm/registry
/// lookup never blocks the rows.
class AgentsPage extends StatefulWidget {
  final MixinController controller;

  /// Widget tests disable this: reading the Windows 启动应用 registry starts a
  /// real process, which the fake-async test binding cannot complete.
  final bool checkStartupAtLogin;

  const AgentsPage({
    super.key,
    required this.controller,
    this.checkStartupAtLogin = true,
  });

  @override
  State<AgentsPage> createState() => _AgentsPageState();
}

class _AgentsPageState extends State<AgentsPage> {
  bool _busy = false;
  bool _foldOpen = false;
  String? _pending;
  final Set<String> _expanded = {};

  MixinController get _controller => widget.controller;
  List<AgentInfo> get _agents => _controller.agents;
  bool get _hasUpdates => _agents.any((agent) => agent.hasUpdate);

  @override
  void initState() {
    super.initState();
    if (widget.checkStartupAtLogin) {
      unawaited(
        _controller.refreshLaunchAtLogin().then((_) {
          if (mounted) setState(() {});
        }),
      );
    }
    unawaited(_load());
  }

  Future<void> _load() async {
    await _controller.refreshAgents();
    if (!mounted) return;
    setState(() {});
    unawaited(_loadVersions());
  }

  Future<void> _loadVersions() async {
    await _controller.refreshAgentVersions();
    if (!mounted) return;
    setState(() {});
  }

  Future<void> _refreshAll() async {
    if (_busy) return;
    setState(() => _busy = true);
    await _controller.refreshAgents();
    if (!mounted) return;
    setState(() => _busy = false);
    unawaited(_loadVersions());
  }

  Future<void> _toggleAgent(AgentInfo agent, bool connect) async {
    if (_busy) return;
    if (connect) {
      // Codex keeps the explicit account choice on every connect from the
      // unmanaged state; the other agents have a single integration mode.
      final mode = agent.id == 'codex' ? await _chooseCodexMode() : null;
      if (agent.id == 'codex' && (mode == null || !mounted)) return;
      await _runConnect(agent, mode);
      return;
    }
    await _runDisconnect(agent);
  }

  Future<void> _runConnect(AgentInfo agent, String? mode) async {
    setState(() {
      _busy = true;
      _pending = agent.id;
    });
    final title = '接入 ${agent.name}';
    await runWithProgress(
      context,
      title: title,
      resultText: (result) =>
          result.ok ? '已接入 ${agent.name}' : '$title失败：${_errorText(result)}',
      action: (onProgress) async {
        final result = await _controller.connectAgent(
          agent.id,
          mode: mode,
          onProgress: onProgress,
        );
        // Re-read the list so the rows always show the CLI's own verdict.
        await _controller.refreshAgents();
        return result;
      },
    );
    if (!mounted) return;
    setState(() {
      _busy = false;
      _pending = null;
    });
    unawaited(_loadVersions());
  }

  Future<void> _runDisconnect(AgentInfo agent) async {
    final confirmed = await confirmAction(
      context,
      title: '断开 ${agent.name} 接入？',
      message:
          '会恢复接入前备份的 ${agent.name} 配置。共享网关继续运行，其他 Agent 不受影响；'
          '客户端需要重启或开新会话后生效。',
      confirmLabel: '断开',
    );
    if (confirmed != true || !mounted) return;
    setState(() {
      _busy = true;
      _pending = agent.id;
    });
    final title = '断开 ${agent.name} 接入';
    await runWithProgress(
      context,
      title: title,
      resultText: (result) =>
          result.ok ? '已断开 ${agent.name} 接入' : '$title失败：${_errorText(result)}',
      action: (onProgress) async {
        final result = await _controller.disconnectAgent(
          agent.id,
          onProgress: onProgress,
        );
        await _controller.refreshAgents();
        return result;
      },
    );
    if (!mounted) return;
    setState(() {
      _busy = false;
      _pending = null;
    });
    unawaited(_loadVersions());
  }

  Future<void> _upgrade(AgentInfo agent) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _pending = agent.id;
    });
    final title = '升级 ${agent.name}';
    await runWithProgress(
      context,
      title: title,
      resultText: (result) => _upgradeResultText(agent, result),
      action: (onProgress) async {
        final result = await _controller.upgradeAgent(
          agent.id,
          onProgress: onProgress,
        );
        await _controller.refreshAgents();
        return result;
      },
    );
    if (!mounted) return;
    setState(() {
      _busy = false;
      _pending = null;
    });
    unawaited(_loadVersions());
  }

  Future<void> _upgradeAll() async {
    if (_busy) return;
    setState(() => _busy = true);
    await runWithProgress(
      context,
      title: '顺序升级全部 Agent',
      resultText: (result) => upgradeAllSummary(result, _agentName),
      action: (onProgress) async {
        final result = await _controller.upgradeAllAgents(
          onProgress: onProgress,
        );
        await _controller.refreshAgents();
        return result;
      },
    );
    if (!mounted) return;
    setState(() => _busy = false);
    unawaited(_loadVersions());
  }

  String _agentName(String id) =>
      _agents.where((agent) => agent.id == id).firstOrNull?.name ?? id;

  Future<String?> _chooseCodexMode() => showDialog<String>(
    context: context,
    builder: (_) => const _CodexModeDialog(),
  );

  String _errorText(CliResult result) {
    final detail = formatCliReport(result.output).trim();
    return detail.isEmpty ? '未返回具体原因' : detail;
  }

  /// A successful exit is not a successful upgrade: the CLI reports a refusal
  /// (unknown install source, missing CLI, a running update) and a failed or
  /// ineffective update in `cli.error` while still exiting 0.
  String _upgradeResultText(AgentInfo agent, CliResult result) {
    if (!result.ok) return '升级 ${agent.name}失败：${_errorText(result)}';
    final cli = _agentCliFromResult(result);
    final error = cli?.error;
    if (error != null) return '升级 ${agent.name}未完成：$error';
    final version = cli?.displayVersion;
    return '已升级 ${agent.name}${version == null ? '' : ' 到 $version'}';
  }

  @override
  Widget build(BuildContext context) {
    final detected = _agents.where((agent) => agent.detected).toList();
    final undetected = _agents.where((agent) => !agent.detected).toList();
    return Scaffold(
      body: Column(
        children: [
          GestureDetector(
            onPanStart: (_) => windowManager.startDragging(),
            child: MixinTitleBar(
              title: 'Agent 接入管理',
              status: _controller.agentsError,
              statusColor: _danger,
              onClose: windowManager.close,
            ),
          ),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 2, 16, 10),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _header(detected.length, undetected.length),
                  const SizedBox(height: 8),
                  Expanded(child: _list(detected, undetected)),
                  const SizedBox(height: 4),
                  _startupFooter(),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _header(int detected, int undetected) => Row(
    children: [
      Expanded(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              undetected == 0
                  ? '已检测到 $detected 个 Agent'
                  : '已检测到 $detected 个 Agent · $undetected 个未检测到',
              style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
            ),
            const SizedBox(height: 2),
            Text(
              _versionStatusText(),
              style: const TextStyle(fontSize: 11.5, color: muted),
            ),
          ],
        ),
      ),
      TextButton(
        onPressed: _busy ? null : _refreshAll,
        child: const Text('刷新'),
      ),
      const SizedBox(width: 6),
      FilledButton(
        onPressed: (_busy || !_hasUpdates) ? null : _upgradeAll,
        child: const Text('全部升级'),
      ),
    ],
  );

  String _versionStatusText() {
    if (_controller.agentVersionsLoading) return '正在异步读取 CLI / App 版本…';
    final error = _controller.agentVersionsError;
    if (error != null) {
      final detail = formatCliReport(error).trim();
      return '版本信息读取失败：${detail.isEmpty ? '未返回具体原因' : detail}';
    }
    final managed = _agents.where((agent) => agent.managed).length;
    return '共 ${_agents.length} 个 Agent · $managed 个已接入';
  }

  Widget _list(List<AgentInfo> detected, List<AgentInfo> undetected) {
    if (!_controller.agentsLoaded) {
      return const Center(
        child: SizedBox(
          width: 26,
          height: 26,
          child: CircularProgressIndicator(strokeWidth: 2.6),
        ),
      );
    }
    if (_controller.agentsError != null && _agents.isEmpty) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.error_outline, size: 40, color: _danger),
            const SizedBox(height: 10),
            const Text(
              '读取 Agent 列表失败',
              style: TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
            ),
            const SizedBox(height: 6),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 24),
              child: Text(
                _controller.agentsError!,
                textAlign: TextAlign.center,
                style: const TextStyle(fontSize: 12, color: muted, height: 1.5),
              ),
            ),
            const SizedBox(height: 12),
            FilledButton(
              onPressed: _busy ? null : _refreshAll,
              child: const Text('重试'),
            ),
          ],
        ),
      );
    }
    if (_agents.isEmpty) {
      return const Center(
        child: Text('没有可管理的 Agent', style: TextStyle(color: muted)),
      );
    }
    return ListView(
      children: [
        for (final agent in detected) _agentRow(agent),
        if (undetected.isNotEmpty) ...[
          const SizedBox(height: 4),
          _foldHeader(undetected.length),
          if (_foldOpen)
            for (final agent in undetected) _agentRow(agent, dimmed: true),
        ],
      ],
    );
  }

  Widget _foldHeader(int count) => HoverButton(
    key: const ValueKey('agent-fold-undetected'),
    borderRadius: 10,
    padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
    onTap: () => setState(() => _foldOpen = !_foldOpen),
    child: Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Icon(
          _foldOpen
              ? Icons.keyboard_arrow_up_rounded
              : Icons.keyboard_arrow_down_rounded,
          size: 18,
          color: muted,
        ),
        const SizedBox(width: 6),
        Text(
          '未检测到的 Agent（$count）',
          style: const TextStyle(fontSize: 12.5, color: muted),
        ),
      ],
    ),
  );

  Widget _agentRow(AgentInfo agent, {bool dimmed = false}) {
    final expanded = _expanded.contains(agent.id);
    final pending = _pending == agent.id;
    return Container(
      key: ValueKey('agent-row-${agent.id}'),
      margin: const EdgeInsets.only(bottom: 6),
      decoration: BoxDecoration(
        color: dimmed ? const Color(0xfff4f5f7) : Colors.white,
        borderRadius: BorderRadius.circular(11),
        border: Border.all(color: line),
      ),
      child: Column(
        children: [
          HoverButton(
            borderRadius: 10,
            padding: const EdgeInsets.fromLTRB(12, 7, 8, 7),
            onTap: () => setState(
              () => expanded
                  ? _expanded.remove(agent.id)
                  : _expanded.add(agent.id),
            ),
            child: Row(
              children: [
                Icon(
                  _agentIcon(agent.id),
                  size: 19,
                  color: dimmed ? muted : const Color(0xff3f4247),
                ),
                const SizedBox(width: 10),
                Expanded(child: _rowTitle(agent, dimmed)),
                const SizedBox(width: 8),
                _versionCell(agent),
                if (pending)
                  const Padding(
                    padding: EdgeInsets.symmetric(horizontal: 8),
                    child: SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    ),
                  ),
                if (!pending && agent.hasUpdate)
                  TextButton(
                    key: ValueKey('agent-upgrade-${agent.id}'),
                    onPressed: _busy ? null : () => _upgrade(agent),
                    child: Text(_upgradeLabel(agent)),
                  ),
                Transform.scale(
                  scale: .78,
                  alignment: Alignment.centerRight,
                  child: Switch(
                    key: ValueKey('agent-switch-${agent.id}'),
                    materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                    value: agent.managed,
                    onChanged: (!agent.detected || _busy)
                        ? null
                        : (value) => _toggleAgent(agent, value),
                  ),
                ),
                Icon(
                  expanded
                      ? Icons.keyboard_arrow_up_rounded
                      : Icons.keyboard_arrow_down_rounded,
                  size: 18,
                  color: muted,
                ),
              ],
            ),
          ),
          if (expanded) _details(agent),
        ],
      ),
    );
  }

  Widget _rowTitle(AgentInfo agent, bool dimmed) => Row(
    children: [
      Flexible(
        child: Text(
          agent.name,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(
            fontSize: 13.5,
            fontWeight: FontWeight.w600,
            color: dimmed ? muted : const Color(0xff1d2024),
          ),
        ),
      ),
      const SizedBox(width: 8),
      _stateBadge(agent),
      if (!dimmed && agent.integration.modeLabel != null) ...[
        const SizedBox(width: 6),
        Flexible(
          child: Text(
            agent.integration.modeLabel!,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 11.5, color: muted),
          ),
        ),
      ],
    ],
  );

  Widget _stateBadge(AgentInfo agent) {
    if (!agent.detected) return statusBadge('未检测到', muted);
    final (label, color) = switch (agent.integration.state) {
      AgentIntegrationState.managed => ('已接入', green),
      AgentIntegrationState.error => ('检测错误', _danger),
      AgentIntegrationState.unmanaged => ('未接入', muted),
    };
    return statusBadge(label, color);
  }

  String _upgradeLabel(AgentInfo agent) {
    final latest = agent.cli?.latest;
    return latest == null ? '升级' : '升级 $latest';
  }

  Widget _versionCell(AgentInfo agent) {
    final cli = agent.cli;
    var color = muted;
    final String text;
    if (!agent.detected) {
      text = '—';
    } else if (cli == null) {
      text = _controller.agentVersionsLoading ? '读取中…' : '版本未知';
    } else if (cli.displayVersion == null && cli.error != null) {
      text = '版本读取失败';
      color = _danger;
    } else if (cli.upgradable) {
      text = '${cli.displayVersion ?? '未知'} → ${cli.latest}';
      color = orange;
    } else {
      text = cli.displayVersion ?? '版本未知';
    }
    return Tooltip(
      message: _versionTooltip(agent),
      child: SizedBox(
        width: 142,
        child: Text(
          text,
          textAlign: TextAlign.right,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(
            fontSize: 11.5,
            fontWeight: FontWeight.w600,
            color: color,
          ),
        ),
      ),
    );
  }

  String _versionTooltip(AgentInfo agent) {
    final cli = agent.cli;
    final lines = <String>[
      '程序检测：${agent.detected ? '已检测到' : '未检测到'}',
    ];
    if (cli != null) {
      if (cli.version != null) lines.add('CLI 版本：${cli.version}');
      if (cli.appVersion != null) lines.add('App 版本：${cli.appVersion}');
      if (cli.latest != null) lines.add('最新版本：${cli.latest}');
      if (cli.via != null) lines.add('安装方式：${cli.via}');
      if (cli.path != null) lines.add('程序路径：${cli.path}');
      if (cli.error != null) lines.add('版本检测：${cli.error}');
    }
    return lines.join('\n');
  }

  Widget _details(AgentInfo agent) {
    final cli = agent.cli;
    final integration = agent.integration;
    final cliVersion = cli?.version ?? '未知';
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.fromLTRB(10, 0, 10, 10),
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 8),
      decoration: BoxDecoration(
        color: const Color(0xfff6f7f9),
        borderRadius: BorderRadius.circular(9),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _detailRow('程序检测', agent.detected ? '已检测到' : '未检测到'),
          _detailRow(
            '接入状态',
            [
              integration.stateLabel,
              if (integration.modeLabel != null) integration.modeLabel!,
            ].join(' · '),
          ),
          if (integration.detail.isNotEmpty)
            _detailRow('接入详情', integration.detail),
          if (integration.error != null)
            _detailRow('接入错误', integration.error!, color: _danger),
          _detailRow('CLI 版本', cliVersion),
          if (cli != null) ...[
            if (cli.appVersion != null)
              _detailRow('App 版本', cli.appVersion!),
            if (cli.latest != null) _detailRow('最新版本', cli.latest!),
            if (cli.via != null) _detailRow('安装方式', cli.via!),
            if (cli.path != null) _detailRow('程序路径', cli.path!),
            if (cli.command != null) _detailRow('升级命令', cli.command!),
            if (cli.error != null)
              _detailRow('版本检测', cli.error!, color: orange),
          ],
          if (agent.hasUpdate)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                '可升级：${cli?.displayVersion ?? '未知'} → ${cli?.latest}',
                style: const TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  color: orange,
                ),
              ),
            ),
        ],
      ),
    );
  }

  Widget _detailRow(String label, String value, {Color? color}) => Padding(
    padding: const EdgeInsets.only(bottom: 4),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 72,
          child: Text(
            label,
            style: const TextStyle(fontSize: 11.5, color: muted),
          ),
        ),
        Expanded(
          child: SelectableText(
            value,
            style: TextStyle(
              fontSize: 11.5,
              height: 1.45,
              color: color ?? const Color(0xff3f4247),
            ),
          ),
        ),
      ],
    ),
  );

  Widget _startupFooter() {
    final text = !_controller.launchAtLoginSupported
        ? '登录时自动启动：当前系统不支持'
        : _controller.launchAtLoginDisabledBySystem
        ? '登录时自动启动：已由 Windows「启动应用」禁用'
        : _controller.launchAtLogin
        ? '登录时自动启动：已启用（默认）'
        : '登录时自动启动：未启用';
    return Row(
      children: [
        const Icon(Icons.login, size: 14, color: muted),
        const SizedBox(width: 6),
        Expanded(
          child: Text(
            text,
            style: const TextStyle(fontSize: 11, color: muted),
          ),
        ),
      ],
    );
  }
}

/// The `cli` object of an `agent upgrade ... --json` reply.
AgentCliInfo? _agentCliFromResult(CliResult result) {
  final json = decodeCliObject(result.stdout);
  final cli = json?['cli'];
  return cli is Map
      ? AgentCliInfo.fromJson(Map<String, dynamic>.from(cli))
      : null;
}

/// Summarize `agent upgrade --all --json` for the result dialog. Failures are
/// named per agent, so a partial upgrade is never reported as plain success.
String upgradeAllSummary(CliResult result, String Function(String id) nameFor) {
  if (!result.ok) {
    final detail = formatCliReport(result.output).trim();
    return '全部升级失败：${detail.isEmpty ? '未返回具体原因' : detail}';
  }
  final json = decodeCliObject(result.stdout);
  final rows = json?['results'];
  if (rows is! List) return '全部升级完成';
  final upgraded = <String>[];
  final failed = <String>[];
  for (final row in rows.whereType<Map>()) {
    final id = '${row['id'] ?? ''}';
    final error = _rowError(row);
    if (error == null) {
      upgraded.add(nameFor(id));
      continue;
    }
    failed.add('${nameFor(id)}：$error');
  }
  if (failed.isEmpty) {
    return upgraded.isEmpty ? '全部升级完成' : '已升级：${upgraded.join('、')}';
  }
  final head = upgraded.isEmpty ? '' : '已升级：${upgraded.join('、')}\n';
  return '$head升级失败 ${failed.length} 个：\n${failed.join('\n')}';
}

/// A row error is either the explicit `error` field or the `cli.error` the
/// CLI fills in when the upgrade was refused or did not take effect.
String? _rowError(Map<dynamic, dynamic> row) {
  final direct = _textOrNull(row['error']);
  if (direct != null) return direct;
  final cli = row['cli'];
  return cli is Map ? _textOrNull(cli['error']) : null;
}

String? _textOrNull(Object? value) {
  if (value is! String) return null;
  final text = value.trim();
  return text.isEmpty ? null : text;
}

class _CodexMode {
  final String value;
  final String title;
  final String description;
  const _CodexMode(this.value, this.title, this.description);
}

const _codexModes = <_CodexMode>[
  _CodexMode(
    'official-account',
    '官方账号模式（推荐：已登录 Codex）',
    '前提是已在官方 Codex App 登录并打开过一次。保留官方认证、GPT、插件、云任务和账户功能；'
        '自定义模型经本地网关加入同一个模型选择器。',
  ),
  _CodexMode(
    'custom-only',
    '仅自定义模型模式（没有官方登录也可用）',
    '用本地 Bedrock 形态的占位身份开启模型选择器。请求只到本地网关，不连接 AWS；'
        '官方插件、云任务和账户功能不可用。',
  ),
];

/// First Codex connect keeps the explicit account-mode choice.
class _CodexModeDialog extends StatefulWidget {
  const _CodexModeDialog();

  @override
  State<_CodexModeDialog> createState() => _CodexModeDialogState();
}

class _CodexModeDialogState extends State<_CodexModeDialog> {
  String? _mode;

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('接入 Codex', textAlign: TextAlign.center),
    content: SizedBox(
      width: 460,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            '选择接入模式。之后可以在本页断开，再改用另一种模式。',
            style: TextStyle(fontSize: 12.5, color: muted, height: 1.5),
          ),
          const SizedBox(height: 12),
          for (final mode in _codexModes) _modeTile(mode),
        ],
      ),
    ),
    actionsAlignment: MainAxisAlignment.center,
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      FilledButton(
        onPressed: _mode == null ? null : () => Navigator.pop(context, _mode),
        child: const Text('接入'),
      ),
    ],
  );

  Widget _modeTile(_CodexMode mode) {
    final selected = _mode == mode.value;
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: GestureDetector(
        key: ValueKey('codex-mode-${mode.value}'),
        onTap: () => setState(() => _mode = mode.value),
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          padding: const EdgeInsets.all(12),
          decoration: BoxDecoration(
            color: selected ? const Color(0xffeef4ff) : const Color(0xfffafbfc),
            border: Border.all(
              color: selected ? accent : line,
              width: selected ? 1.5 : 1,
            ),
            borderRadius: BorderRadius.circular(11),
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _radioDot(selected),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      mode.title,
                      style: TextStyle(
                        fontSize: 13,
                        height: 1.35,
                        fontWeight: FontWeight.w600,
                        color: selected ? accent : const Color(0xff1d2024),
                      ),
                    ),
                    const SizedBox(height: 5),
                    Text(
                      mode.description,
                      style: const TextStyle(
                        color: muted,
                        fontSize: 12,
                        height: 1.5,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _radioDot(bool selected) => Container(
    margin: const EdgeInsets.only(top: 2),
    width: 17,
    height: 17,
    decoration: BoxDecoration(
      shape: BoxShape.circle,
      border: Border.all(
        color: selected ? accent : const Color(0xffbcc2cc),
        width: 2,
      ),
    ),
    child: selected
        ? Center(
            child: Container(
              width: 7,
              height: 7,
              decoration: const BoxDecoration(
                shape: BoxShape.circle,
                color: accent,
              ),
            ),
          )
        : null,
  );
}

MaterialApp agentsApp({
  required MixinController controller,
  bool checkStartupAtLogin = true,
}) {
  return MaterialApp(
    debugShowCheckedModeBanner: false,
    title: 'Agent 接入管理',
    theme: mixinTheme(),
    locale: mixinLocale,
    supportedLocales: mixinSupportedLocales,
    localizationsDelegates: mixinLocalizationsDelegates,
    home: AgentsPage(
      controller: controller,
      checkStartupAtLogin: checkStartupAtLogin,
    ),
  );
}
