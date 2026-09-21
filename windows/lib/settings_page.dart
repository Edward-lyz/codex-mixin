import 'dart:async';

import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'controller.dart';
import 'cli.dart';
import 'models.dart';
import 'provider_form.dart';
import 'theme.dart';
import 'widgets.dart';

class SettingsPage extends StatefulWidget {
  final MixinController controller;
  final bool autoRefresh;
  final bool closeHides;

  const SettingsPage({
    super.key,
    required this.controller,
    this.autoRefresh = true,
    this.closeHides = true,
  });

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> with WindowListener {
  final _displayController = TextEditingController();
  final _baseController = TextEditingController();
  final _websiteController = TextEditingController();
  final _imageController = TextEditingController();
  final _workspaceController = TextEditingController();
  final _authCookieController = TextEditingController();
  final _awsRegionController = TextEditingController(text: 'us-east-1');

  String? _selectedId;
  bool _sidebarVisible = true;
  bool _auxiliary = false;
  bool _baiduReport = false;
  bool _baiduAuthBridge = false;
  String _protocol = 'open_ai_responses';
  int _detailTab = 1;
  final Set<String> _selectedModels = {};
  String _modelQuery = '';
  Timer? _refreshTimer;

  // Per-model benchmark state (merged from the standalone benchmark window):
  // a test-mode selector (target output tokens 1 = latency, 100 = full), a
  // timeout selector, and the latest results keyed by upstream model id.
  int _benchMode = 1;
  int _benchTimeout = 5;
  bool _benchBusy = false;
  Map<String, Map<String, dynamic>> _benchResults = {};
  Timer? _benchPoll;

  MixinController get _controller => widget.controller;
  List<ProviderModel> get _providers => _controller.snapshot.providers;
  ProviderModel? get _selected =>
      _providers.where((p) => p.id == _selectedId).firstOrNull;
  bool get _compact => MediaQuery.sizeOf(context).width < 850;
  bool get _busy => _controller.busy;

  @override
  void initState() {
    super.initState();
    windowManager.addListener(this);
    _selectedId = _providers.firstOrNull?.id;
    _loadSelectedIntoForm();
    if (widget.autoRefresh) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _refresh());
      _refreshTimer = Timer.periodic(
        const Duration(seconds: 10),
        (_) => _refresh(),
      );
    }
  }

  @override
  void dispose() {
    for (final controller in [
      _displayController,
      _baseController,
      _websiteController,
      _imageController,
      _workspaceController,
      _authCookieController,
      _awsRegionController,
    ]) {
      controller.dispose();
    }
    windowManager.removeListener(this);
    _refreshTimer?.cancel();
    _benchPoll?.cancel();
    super.dispose();
  }

  @override
  void onWindowClose() {
    if (widget.closeHides) {
      unawaited(windowManager.hide());
    }
  }

  Future<void> _refresh() async {
    await _controller.refresh();
    if (!mounted) return;
    setState(() {
      if (_selectedId == null ||
          !_providers.any((provider) => provider.id == _selectedId)) {
        _selectedId = _providers.firstOrNull?.id;
        _loadSelectedIntoForm();
      }
      // Do NOT reload the edit form for the still-selected provider here: the
      // periodic refresh would otherwise clobber the user's unsaved edits
      // (e.g. toggling 上报 AI 代码使用数据 / 辅助模型路由 back off a few seconds
      // after they turn it on). Form state is loaded on provider switch and
      // after an explicit save instead.
    });
  }

  void _loadSelectedIntoForm() {
    final provider = _selected;
    if (provider == null) return;
    _displayController.text = provider.displayName;
    _baseController.text = provider.baseUrl;
    _websiteController.text = provider.websiteUrl ?? '';
    _imageController.text = provider.imageGenerationPath ?? '';
    _workspaceController.text = provider.quotaWorkspaceId ?? '';
    _authCookieController.clear();
    _awsRegionController.text = provider.awsRegion ?? 'us-east-1';
    _protocol = provider.protocol;
    _auxiliary = provider.auxiliaryModelUpstream;
    _baiduReport = provider.baiduCodeReport == true;
    _baiduAuthBridge = provider.baiduAuthBridge == 'ducx_loopback';
    _selectedModels
      ..clear()
      ..addAll(provider.selectedModels);
    // Benchmark results belong to the previously selected provider; drop them
    // so a switch does not show stale numbers against the new model list.
    _benchPoll?.cancel();
    _benchBusy = false;
    _benchResults = {};
  }

  Future<CliResult> _run(
    String label,
    List<String> args, {
    Map<String, String>? secrets,
    String Function(CliResult result)? resultText,
  }) async {
    setState(() {});
    final result = await runWithProgress(
      context,
      title: label,
      resultText: resultText,
      action: (onProgress) async {
        final result = await _controller.runAction(
          label,
          args,
          secrets: secrets,
          // Adding/saving a Baidu provider can trigger an interactive DUCX
          // login in a separate window, so allow generous time before the
          // action is treated as hung.
          timeout: const Duration(minutes: 20),
          onProgress: onProgress,
        );
        if (result.ok) {
          // Best-effort restart so the runtime picks up the change. A restart
          // hiccup must NOT mark the already-committed mutation as failed.
          final restart = await _controller.cli.run(['service', 'restart']);
          if (!restart.ok) {
            _controller.status = '$label成功，但重启网关失败：${restart.output}';
          }
        }
        try {
          await _controller
              .refresh(force: true)
              .timeout(const Duration(seconds: 15));
        } catch (error) {
          // The mutation result is still authoritative. Keep it available to
          // the caller when a secondary status refresh fails.
          _controller.status = '$label已执行，但刷新配置失败：$error';
        }
        return result;
      },
    );
    if (mounted) {
      setState(() {
        if (result.ok) _loadSelectedIntoForm();
      });
    }
    return result;
  }

  Future<void> _saveProvider() async {
    final provider = _selected;
    if (provider == null || provider.official) return;
    final args = ['provider', 'update', provider.id];
    final secrets = <String, String>{};
    // Route credentials through the environment (see MixinCli.run) instead of
    // argv so they never surface on the Windows command line.
    void addSecret(String flag, String envName, String value) {
      args.addAll([flag, '@env:$envName']);
      secrets[envName] = value;
    }

    if (provider.isCustom) {
      args.addAll([
        '--display-name',
        _displayController.text.trim(),
        '--base-url',
        _baseController.text.trim(),
        '--protocol',
        _protocol,
      ]);
      if (_websiteController.text.trim().isNotEmpty) {
        args.addAll(['--website-url', _websiteController.text.trim()]);
      }
      if (_imageController.text.trim().isNotEmpty) {
        args.addAll(['--image-generation-path', _imageController.text.trim()]);
      }
    }
    if (provider.isAwsBedrock) {
      // Credentials are no longer editable inline for an existing provider;
      // only the non-secret region is saved here. Keys are managed via add or
      // the 清除凭据 button.
      if (_awsRegionController.text.trim().isNotEmpty) {
        args.addAll(['--aws-region', _awsRegionController.text.trim()]);
      }
    }
    // The API key is never sent on save for an existing provider; it is managed
    // only via add (new provider) or the 清除密钥 button.
    if (provider.isBaiduOneApi) {
      args.addAll([
        '--baidu-auth-bridge',
        _baiduAuthBridge ? 'ducx_loopback' : 'disabled',
      ]);
      args.addAll(['--baidu-code-report', '$_baiduReport']);
    }
    if (provider.isOpenCodeGo) {
      if (_workspaceController.text.trim().isNotEmpty) {
        args.addAll(['--quota-workspace-id', _workspaceController.text.trim()]);
      }
      if (_authCookieController.text.trim().isNotEmpty) {
        addSecret(
          '--quota-auth-cookie',
          'CM_SEC_COOKIE',
          _authCookieController.text.trim(),
        );
      }
    }
    args.addAll(['--auxiliary-model-upstream', '$_auxiliary']);
    final result = await _run(
      '保存供应商',
      args,
      secrets: secrets,
      resultText: (result) {
        if (!result.ok) {
          final details = formatCliReport(result.output).trim();
          return details.isEmpty ? '保存供应商失败' : '保存供应商失败：$details';
        }
        final saved = _selected;
        final providerSaved = saved != null;
        final reportSaved =
            !provider.isBaiduOneApi || saved?.baiduCodeReport == _baiduReport;
        final auxiliarySaved = saved?.auxiliaryModelUpstream == _auxiliary;
        return !providerSaved || !reportSaved || !auxiliarySaved
            ? '保存供应商失败：保存结果校验失败，请重试'
            : '供应商配置已保存并校验通过';
      },
    );
    if (!result.ok) return;
    if (!mounted) return;
    _authCookieController.clear();
  }

  /// Clear a stored credential (API key or AWS credentials) via the unified
  /// progress dialog, then refresh so the read-only status reflects the change.
  Future<void> _clearCredential(String title, List<String> args) async {
    final provider = _selected;
    if (provider == null || provider.official || _busy) return;
    await _run(
      title,
      args,
      resultText: (result) => result.ok
          ? '$title成功'
          : '$title失败：${formatCliReport(result.output).trim().isEmpty ? '未返回具体原因' : formatCliReport(result.output).trim()}',
    );
  }

  Future<void> _addProvider() async {
    final result = await showDialog<AddProviderValues>(
      context: context,
      builder: (_) => const AddProviderDialog(),
    );
    if (result == null) return;
    if (result.preset != 'aws-bedrock' && result.apiKey.trim().isEmpty) {
      setState(() => _controller.status = '请输入 API Key');
      return;
    }
    if (result.preset == 'aws-bedrock' &&
        (result.awsRegion.trim().isEmpty ||
            result.awsAccessKeyId.trim().isEmpty ||
            result.awsSecretAccessKey.trim().isEmpty)) {
      setState(
        () => _controller.status =
            'Amazon Bedrock 需要填写 AWS Region、Access Key ID 和 Secret Access Key',
      );
      return;
    }
    if (result.preset == 'baidu-oneapi' &&
        result.quotaUsername.trim().isEmpty) {
      setState(() => _controller.status = 'Baidu OneAPI 需要填写额度用户名');
      return;
    }
    if (result.preset == 'opencode-go' &&
        (result.workspaceId.trim().isEmpty ||
            result.authCookie.trim().isEmpty)) {
      setState(
        () => _controller.status = 'OpenCode Go 需要填写工作区 ID 和 Auth Cookie',
      );
      return;
    }
    if (result.preset == 'custom' &&
        (result.name.trim().isEmpty || result.baseUrl.trim().isEmpty)) {
      setState(() => _controller.status = '自定义站点需要填写站点名称和 API 地址');
      return;
    }
    final id = result.preset == 'custom'
        ? result.customId.toLowerCase()
        : result.preset;
    if (result.preset == 'custom' &&
        !RegExp(r'^[a-z0-9_-]{1,64}$').hasMatch(id)) {
      setState(() => _controller.status = '供应商 ID 只能包含小写英文、数字、- 或 _，且不能为空');
      return;
    }
    final args = <String>[
      'provider',
      'add',
      '--preset',
      result.preset,
      '--id',
      id,
    ];
    final secrets = <String, String>{};
    // Credentials travel via the environment (see MixinCli.run), not argv.
    void addSecret(String flag, String envName, String value) {
      args.addAll([flag, '@env:$envName']);
      secrets[envName] = value;
    }

    if (result.preset == 'aws-bedrock') {
      addSecret(
        '--aws-access-key-id',
        'CM_SEC_AWS_ACCESS',
        result.awsAccessKeyId,
      );
      addSecret(
        '--aws-secret-access-key',
        'CM_SEC_AWS_SECRET',
        result.awsSecretAccessKey,
      );
      args.addAll(['--aws-region', result.awsRegion]);
      if (result.awsSessionToken.isNotEmpty) {
        addSecret(
          '--aws-session-token',
          'CM_SEC_AWS_SESSION',
          result.awsSessionToken,
        );
      }
    } else {
      addSecret('--key', 'CM_SEC_KEY', result.apiKey);
    }
    if (result.preset == 'custom') {
      args.addAll([
        '--display-name',
        result.name,
        '--base-url',
        result.baseUrl,
      ]);
      if (result.websiteUrl.isNotEmpty) {
        args.addAll(['--website-url', result.websiteUrl]);
      }
    }
    if (result.preset == 'baidu-oneapi') {
      args.addAll(['--quota-username', result.quotaUsername]);
      args.addAll([
        '--baidu-auth-bridge',
        result.baiduAuthBridge ? 'ducx_loopback' : 'disabled',
      ]);
    }
    if (result.preset == 'opencode-go') {
      args.addAll(['--quota-workspace-id', result.workspaceId]);
      addSecret('--quota-auth-cookie', 'CM_SEC_COOKIE', result.authCookie);
    }
    final addResult = await _run(
      '新增供应商',
      args,
      secrets: secrets,
      resultText: (result) {
        if (!result.ok) {
          final details = formatCliReport(result.output).trim();
          return details.isEmpty ? '新增供应商失败' : '新增供应商失败：$details';
        }
        final added = _providers
            .where((provider) => provider.id == id)
            .firstOrNull;
        return added == null
            ? '新增供应商失败：命令执行成功，但未在配置中找到新供应商，请重试'
            : '供应商已添加并完成配置校验';
      },
    );
    if (!mounted) return;
    final added = _providers.where((provider) => provider.id == id).firstOrNull;
    if (!addResult.ok || added == null) {
      if (added != null) {
        setState(() {
          _selectedId = id;
          _loadSelectedIntoForm();
        });
      }
      return;
    }
    setState(() {
      _selectedId = id;
      _loadSelectedIntoForm();
    });
  }

  Future<void> _removeProvider() async {
    final provider = _selected;
    if (provider == null || provider.official) return;
    final confirmed = await confirmAction(
      context,
      title: '删除 ${provider.displayName}？',
      message: '这会删除供应商的地址、密钥和模型选择。',
      confirmLabel: '删除',
    );
    if (!confirmed) return;
    final removedId = provider.id;
    final result = await _run(
      '删除供应商',
      ['provider', 'remove', removedId],
      resultText: (result) {
        if (!result.ok) {
          final details = formatCliReport(result.output).trim();
          return details.isEmpty ? '删除供应商失败' : '删除供应商失败：$details';
        }
        return _providers.any((item) => item.id == removedId)
            ? '删除供应商失败：命令执行成功，但供应商仍存在，请重试'
            : '供应商已删除';
      },
    );
    if (!mounted) return;
    final stillExists = _providers.any((item) => item.id == removedId);
    if (!result.ok || stillExists) return;
    setState(() {
      _selectedId = _providers.firstOrNull?.id;
      _loadSelectedIntoForm();
    });
  }

  Future<void> _saveSelectedModels() async {
    final provider = _selected;
    if (provider == null || provider.official) return;
    final args = ['provider', 'select', provider.id];
    for (final model in _selectedModels) {
      args.addAll(['--model', model]);
    }
    final result = await _run('保存模型选择', args);
    if (!result.ok) return;
    await _controller.cli.run(['service', 'restart']);
    await _controller.cli.run(['refresh-codex-catalog']);
    await _controller.refresh(force: true);
    if (mounted) setState(_loadSelectedIntoForm);
  }

  Future<void> _refreshModels() async {
    final provider = _selected;
    if (provider == null) return;
    await _run('刷新模型', ['provider', 'discover', provider.id]);
  }

  // ---- Benchmark (merged model speed test) --------------------------------

  Color _benchStatusColor(String status) {
    switch (status) {
      case 'completed':
        return green;
      case 'running':
        return accent;
      case 'timed_out':
        return orange;
      case 'failed':
      case 'interrupted':
        return const Color(0xffc62828);
      default:
        return muted;
    }
  }

  String _benchStatusLabel(String status) {
    switch (status) {
      case 'completed':
        return '成功';
      case 'running':
        return '测试中';
      case 'timed_out':
        return '超时';
      case 'failed':
      case 'interrupted':
        return '失败';
      default:
        return status;
    }
  }

  /// Start a benchmark for the whole provider or a single [modelId], then poll
  /// `benchmark status` every 2s and merge the results into the model rows.
  Future<void> _startBenchmark() async {
    var provider = _selected;
    if (provider == null || _benchBusy) return;
    setState(() {
      _benchBusy = true;
      _benchResults = {};
    });
    // A provider can be present in config while its selected model cache is
    // empty; refresh it before asking the gateway to build benchmark targets.
    if (provider.selectedModels.isEmpty || provider.routableModelCount == 0) {
      final discovery = await _controller.cli.run([
        'provider',
        'discover',
        provider.id,
      ]);
      if (discovery.ok) {
        await _controller.refresh(force: true);
        provider = _selected;
      }
    }
    if (!mounted) return;
    if (provider == null ||
        provider.selectedModels.isEmpty ||
        provider.routableModelCount == 0) {
      setState(() {
        _benchBusy = false;
        _controller.status = '启动测速失败：当前供应商没有可用模型，请先刷新并选择模型';
      });
      return;
    }
    // Run the benchmark for the whole provider. Passing a per-model `--model`
    // filter makes the gateway reject the run ("benchmark model is not
    // routable"), so the gateway picks the routable targets itself. The virtual
    // "auto" default model is skipped when the results are applied instead.
    final result = await _controller.cli.run([
      'benchmark',
      'start',
      '--provider',
      provider.id,
      '--timeout-seconds',
      '$_benchTimeout',
      '--target-output-tokens',
      '$_benchMode',
    ]);
    if (!mounted) return;
    if (!result.ok) {
      setState(() {
        _benchBusy = false;
        _controller.status = '启动测速失败：${formatCliReport(result.output)}';
      });
      return;
    }
    _applyBenchSnapshot(decodeCliObject(result.stdout));
    _benchPoll?.cancel();
    _benchPoll = Timer.periodic(
      const Duration(seconds: 2),
      (_) => _pollBenchStatus(),
    );
  }

  Future<void> _pollBenchStatus() async {
    final result = await _controller.cli.run(['benchmark', 'status']);
    if (!mounted) {
      _benchPoll?.cancel();
      return;
    }
    _applyBenchSnapshot(decodeCliObject(result.stdout));
  }

  void _applyBenchSnapshot(Map<String, dynamic>? envelope) {
    if (!mounted) return;
    final snapshot = envelope?['snapshot'] as Map?;
    if (snapshot == null) return;
    final results = <String, Map<String, dynamic>>{};
    for (final raw
        in ((snapshot['results'] as List?) ?? const []).whereType<Map>()) {
      final key = '${raw['upstream_model'] ?? raw['model']}';
      // The virtual "auto" default model is not a real upstream model and always
      // times out; never surface a benchmark result for it.
      if (key == 'auto') continue;
      results[key] = Map<String, dynamic>.from(raw);
    }
    final status = '${snapshot['status'] ?? ''}';
    setState(() {
      _benchResults = results;
      _benchBusy = status == 'running';
    });
    if (status != 'running') _benchPoll?.cancel();
  }

  List<Map<String, dynamic>> get _visibleModels {
    final provider = _selected;
    if (provider == null) return const [];
    final query = _modelQuery.trim().toLowerCase();
    return provider.cachedModels.where((model) {
      if (query.isEmpty) return true;
      final id = '${model['id']}'.toLowerCase();
      final name = '${model['display_name'] ?? ''}'.toLowerCase();
      return id.contains(query) || name.contains(query);
    }).toList();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Column(
        children: [
          GestureDetector(
            onDoubleTap: () async {
              if (await windowManager.isMaximized()) {
                await windowManager.unmaximize();
              } else {
                await windowManager.maximize();
              }
            },
            onPanStart: (_) => windowManager.startDragging(),
            child: MixinTitleBar(
              title: '供应商设置',
              onClose: () => widget.closeHides
                  ? windowManager.hide()
                  : windowManager.close(),
              onMinimize: windowManager.minimize,
              onMaximize: () async {
                if (await windowManager.isMaximized()) {
                  await windowManager.unmaximize();
                } else {
                  await windowManager.maximize();
                }
              },
              leading: _sidebarToggleButton(),
            ),
          ),
          Expanded(
            child: Container(
              color: panel,
              child: _compact || !_sidebarVisible
                  ? _buildDetail(compact: true)
                  : Row(
                      children: [
                        SizedBox(width: 300, child: _buildSidebar()),
                        Expanded(child: _buildDetail()),
                      ],
                    ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _sidebarToggleButton() {
    return FlatIconButton(
      key: const Key('toggle-provider-sidebar'),
      tooltip: _sidebarVisible ? '收起供应商列表' : '显示供应商列表',
      onTap: () => setState(() => _sidebarVisible = !_sidebarVisible),
      icon: _sidebarVisible
          ? Icons.chevron_left_rounded
          : Icons.chevron_right_rounded,
    );
  }

  Widget _buildSidebar() => Container(
    margin: const EdgeInsets.fromLTRB(16, 16, 0, 16),
    decoration: BoxDecoration(
      color: const Color(0xffe9eaeb),
      border: Border.all(color: Colors.white),
      borderRadius: BorderRadius.circular(24),
    ),
    child: Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(18, 16, 16, 10),
          child: Row(
            children: [
              const Expanded(
                child: Text(
                  '供应商',
                  style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
                ),
              ),
              IconButton(
                tooltip: '新增供应商',
                onPressed: _addProvider,
                icon: const Icon(Icons.add),
              ),
              IconButton(
                tooltip: '删除供应商',
                onPressed: _removeProvider,
                icon: const Icon(Icons.remove),
              ),
            ],
          ),
        ),
        Expanded(
          child: _providers.isEmpty
              ? const Center(
                  child: Text('还没有供应商', style: TextStyle(color: muted)),
                )
              : ReorderableListView.builder(
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  itemCount: _providers.length,
                  // ignore: deprecated_member_use
                  onReorder: (oldIndex, newIndex) async {
                    if (newIndex > oldIndex) newIndex--;
                    final list = [..._providers];
                    final item = list.removeAt(oldIndex);
                    list.insert(newIndex, item);
                    final ids = list
                        .where((p) => !p.official)
                        .map((p) => p.id)
                        .toList();
                    await _run('保存供应商顺序', ['provider', 'reorder', ...ids]);
                  },
                  itemBuilder: (context, index) {
                    final provider = _providers[index];
                    final selected = provider.id == _selectedId;
                    return ListTile(
                      key: ValueKey(provider.id),
                      selected: selected,
                      selectedTileColor: const Color(0xffdce8fb),
                      selectedColor: const Color(0xff1d2024),
                      iconColor: const Color(0xff1d2024),
                      textColor: const Color(0xff1d2024),
                      shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(12),
                      ),
                      leading: providerLogo(provider, 40),
                      title: Text(
                        provider.displayName,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          fontWeight: FontWeight.w700,
                          color: Color(0xff1d2024),
                        ),
                      ),
                      subtitle: Text(
                        provider.modelSummary,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(fontSize: 12, color: muted),
                      ),
                      onTap: () => setState(() {
                        _selectedId = provider.id;
                        _loadSelectedIntoForm();
                      }),
                    );
                  },
                ),
        ),
        const SizedBox(height: 16),
      ],
    ),
  );

  Widget _buildDetail({bool compact = false}) {
    final provider = _selected;
    if (provider == null) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.dns_outlined, size: 46, color: Colors.grey.shade400),
            const SizedBox(height: 12),
            const Text(
              '还没有 Provider',
              style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
            ),
            const SizedBox(height: 8),
            const Text(
              '添加上游 API 后，Codex Mixin 才能为网关提供模型。',
              style: TextStyle(color: muted),
            ),
            const SizedBox(height: 18),
            Tooltip(
              message: '新增供应商',
              child: FilledButton(
                onPressed: _addProvider,
                child: const Text('新增 Provider'),
              ),
            ),
          ],
        ),
      );
    }
    return Padding(
      padding: EdgeInsets.fromLTRB(
        compact ? 18 : 42,
        28,
        compact ? 18 : 48,
        24,
      ),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 780),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                providerLogo(provider, compact ? 48 : 54),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Flexible(
                            child: Text(
                              provider.displayName,
                              overflow: TextOverflow.ellipsis,
                              style: const TextStyle(
                                fontSize: 20,
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                          ),
                          const SizedBox(width: 10),
                          statusBadge(
                            provider.stateLabel,
                            provider.statusColor,
                          ),
                        ],
                      ),
                      Text(
                        provider.official
                            ? '官方供应商 · 只读'
                            : '${provider.preset} · ${provider.protocol}',
                        style: const TextStyle(color: muted, fontSize: 13),
                      ),
                    ],
                  ),
                ),
                if (!provider.official)
                  IconButton(
                    tooltip: provider.enabled ? '停用供应商' : '启用供应商',
                    onPressed: () =>
                        _run(provider.enabled ? '停用供应商' : '启用供应商', [
                          'provider',
                          provider.enabled ? 'disable' : 'enable',
                          provider.id,
                        ]),
                    icon: Icon(
                      provider.enabled
                          ? Icons.pause_circle_outline
                          : Icons.play_circle_outline,
                    ),
                  ),
              ],
            ),
            const SizedBox(height: 22),
            _detailTabBar(),
            const SizedBox(height: 18),
            Expanded(
              child: _detailTab == 0
                  ? _modelsPanel(provider)
                  : SingleChildScrollView(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          sectionTitle('连接配置'),
                          const SizedBox(height: 10),
                          sectionBox(_connectionFields(provider)),
                          if (provider.isBaiduOneApi) ...[
                            const SizedBox(height: 24),
                            sectionTitle('百度额度'),
                            const SizedBox(height: 10),
                            sectionBox(
                              Column(
                                children: [
                                  Row(
                                    children: [
                                      const Icon(
                                        Icons.person_outline,
                                        size: 18,
                                        color: muted,
                                      ),
                                      const SizedBox(width: 10),
                                      const Text(
                                        '额度用户名',
                                        style: TextStyle(color: muted),
                                      ),
                                      const Spacer(),
                                      Flexible(
                                        child: Text(
                                          provider.quotaUsername?.isNotEmpty ==
                                                  true
                                              ? provider.quotaUsername!
                                              : '未设置',
                                          maxLines: 1,
                                          overflow: TextOverflow.ellipsis,
                                          textAlign: TextAlign.right,
                                          style: const TextStyle(
                                            fontWeight: FontWeight.w600,
                                          ),
                                        ),
                                      ),
                                    ],
                                  ),
                                  labeledSwitch(
                                    title: '认证桥接（DUCX 登录态）',
                                    value: _baiduAuthBridge,
                                    onChanged: _busy
                                        ? null
                                        : (value) {
                                            setState(
                                              () => _baiduAuthBridge = value,
                                            );
                                            _saveProvider();
                                          },
                                  ),
                                  labeledSwitch(
                                    title: '上报 AI 代码使用数据',
                                    value: _baiduReport,
                                    onChanged: _busy
                                        ? null
                                        : (value) {
                                            setState(
                                              () => _baiduReport = value,
                                            );
                                            _saveProvider();
                                          },
                                  ),
                                ],
                              ),
                            ),
                          ],
                          if (provider.isOpenCodeGo) ...[
                            const SizedBox(height: 24),
                            sectionTitle('OpenCode Go'),
                            const SizedBox(height: 10),
                            sectionBox(
                              Column(
                                children: [
                                  labeledField(
                                    _workspaceController,
                                    '工作区 ID',
                                    Icons.workspaces_outline,
                                    onSubmitted: (_) => _saveProvider(),
                                  ),
                                  const SizedBox(height: 10),
                                  labeledField(
                                    _authCookieController,
                                    provider.quotaAuthCookieConfigured
                                        ? 'Auth Cookie（留空保留现有值）'
                                        : 'Auth Cookie',
                                    Icons.cookie_outlined,
                                    obscure: true,
                                    onSubmitted: (_) => _saveProvider(),
                                  ),
                                ],
                              ),
                            ),
                          ],
                          const SizedBox(height: 24),
                          sectionTitle('辅助模型路由'),
                          const SizedBox(height: 10),
                          sectionBox(
                            labeledSwitch(
                              title: '用于绘图、自动审查等辅助模型上游',
                              value: _auxiliary,
                              onChanged: provider.official || _busy
                                  ? null
                                  : (value) {
                                      setState(() => _auxiliary = value);
                                      _saveProvider();
                                    },
                            ),
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

  Widget _detailTabBar() {
    Widget tab(String label, int index) {
      final selected = _detailTab == index;
      return GestureDetector(
        onTap: () => setState(() => _detailTab = index),
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 140),
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          decoration: BoxDecoration(
            color: selected ? accent : Colors.transparent,
            borderRadius: BorderRadius.circular(20),
          ),
          child: Text(
            label,
            style: TextStyle(
              fontSize: 13,
              fontWeight: FontWeight.w600,
              color: selected ? Colors.white : const Color(0xff3f4247),
            ),
          ),
        ),
      );
    }

    return Align(
      alignment: Alignment.centerLeft,
      child: Container(
        padding: const EdgeInsets.all(3),
        decoration: BoxDecoration(
          color: const Color(0xffe8eaed),
          borderRadius: BorderRadius.circular(22),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [tab('模型', 0), tab('连接设置', 1)],
        ),
      ),
    );
  }

  Widget _modelsPanel(ProviderModel provider) {
    final rows = _visibleModels;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        LayoutBuilder(
          builder: (context, constraints) {
            // Stack the search field above its action buttons when the panel is
            // too narrow to keep them comfortably on one line.
            final stackActions = constraints.maxWidth < 560;
            final search = TextField(
              decoration: const InputDecoration(
                prefixIcon: Icon(Icons.search),
                hintText: '搜索当前供应商的模型',
              ),
              onChanged: (value) => setState(() => _modelQuery = value),
            );
            final actions = Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                OutlinedButton(
                  onPressed: _busy || _benchBusy ? null : _refreshModels,
                  child: const Text('刷新模型'),
                ),
                const SizedBox(width: 10),
                FilledButton(
                  onPressed: provider.official || _busy || _benchBusy
                      ? null
                      : _saveSelectedModels,
                  child: const Text('保存模型选择'),
                ),
              ],
            );
            if (stackActions) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  search,
                  const SizedBox(height: 12),
                  Align(alignment: Alignment.centerRight, child: actions),
                ],
              );
            }
            return Row(
              crossAxisAlignment: CrossAxisAlignment.center,
              children: [
                Expanded(child: search),
                const SizedBox(width: 16),
                actions,
              ],
            );
          },
        ),
        const SizedBox(height: 18),
        _benchmarkToolbar(provider),
        const SizedBox(height: 16),
        Text(
          '${_selectedModels.length}/${provider.cachedModels.length} 个模型已加入 Codex',
          style: const TextStyle(color: muted, fontSize: 12),
        ),
        const SizedBox(height: 12),
        Expanded(
          child: rows.isEmpty
              ? sectionBox(
                  Text(
                    provider.cachedModels.isEmpty
                        ? '当前供应商还没有模型，先刷新模型列表。'
                        : '没有匹配的模型',
                    style: const TextStyle(color: muted),
                  ),
                )
              : sectionBox(
                  ListView.separated(
                    itemCount: rows.length,
                    separatorBuilder: (_, _) => const Divider(height: 1),
                    itemBuilder: (context, index) =>
                        _modelRow(provider, rows[index]),
                  ),
                ),
        ),
      ],
    );
  }

  Widget _benchmarkToolbar(ProviderModel provider) {
    return Wrap(
      spacing: 14,
      runSpacing: 12,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        SizedBox(
          width: 260,
          child: mixinDropdown<int>(
            selected: _benchMode,
            decoration: const InputDecoration(labelText: '测速模式'),
            items: const [
              DropdownMenuItem(value: 1, child: Text('延迟（TTFT）')),
              DropdownMenuItem(value: 100, child: Text('完整（TTFT+吞吐）')),
            ],
            onChanged: _benchBusy
                ? null
                : (value) => setState(() => _benchMode = value ?? 1),
          ),
        ),
        SizedBox(
          width: 120,
          child: mixinDropdown<int>(
            selected: _benchTimeout,
            decoration: const InputDecoration(labelText: '超时'),
            items: const [5, 10, 20, 30, 60]
                .map(
                  (value) =>
                      DropdownMenuItem(value: value, child: Text('$value 秒')),
                )
                .toList(),
            onChanged: _benchBusy
                ? null
                : (value) => setState(() => _benchTimeout = value ?? 5),
          ),
        ),
        FilledButton.icon(
          onPressed: provider.official || _busy || _benchBusy
              ? null
              : () => _startBenchmark(),
          icon: _benchBusy
              ? const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : const Icon(Icons.speed, size: 18),
          label: const Text('测速'),
        ),
      ],
    );
  }

  Widget _modelRow(ProviderModel provider, Map<String, dynamic> model) {
    final id = '${model['id']}';
    final name = '${model['display_name'] ?? ''}';
    final selected = _selectedModels.contains(id);
    // The virtual "auto" default model never gets a real benchmark result, so
    // never show a benchmark status for its row.
    final result = id == 'auto' ? null : _benchResults[id];
    final ttft = result?['ttft_ms'];
    final tps = result?['tps'];
    final status = '${result?['status'] ?? ''}';
    final contextWindow = model['context_window'];
    final ratio = model['ratio'];
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        children: [
          Checkbox(
            value: selected,
            onChanged: provider.official || _busy || _benchBusy
                ? null
                : (checked) => setState(() {
                    if (checked == true) {
                      _selectedModels.add(id);
                    } else {
                      _selectedModels.remove(id);
                    }
                  }),
          ),
          const SizedBox(width: 4),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Flexible(
                      child: Text(
                        id,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(fontWeight: FontWeight.w600),
                      ),
                    ),
                    ..._capabilityIcons(model),
                  ],
                ),
                if (name.isNotEmpty && name != id)
                  Text(
                    name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(color: muted, fontSize: 12),
                  ),
                if (status.isNotEmpty)
                  Text(
                    _benchStatusLabel(status),
                    style: TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w600,
                      color: _benchStatusColor(status),
                    ),
                  ),
              ],
            ),
          ),
          _statCell('首Token', ttft is num ? '$ttft ms' : '-'),
          _statCell('生成速度', tps is num ? '$tps tok/s' : '-'),
          _statCell(
            '上下文',
            contextWindow is num ? '${(contextWindow / 1000).round()}K' : '-',
          ),
          _statCell('倍率', _ratioText(ratio)),
        ],
      ),
    );
  }

  Widget _statCell(String label, String value) {
    return SizedBox(
      width: 64,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label, style: const TextStyle(color: muted, fontSize: 10)),
          const SizedBox(height: 2),
          Text(
            value,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 12, fontWeight: FontWeight.w600),
          ),
        ],
      ),
    );
  }

  List<Widget> _capabilityIcons(Map<String, dynamic> model) {
    final icons = <Widget>[];
    void add(Object? enabled, IconData icon, String label) {
      if (enabled == true) {
        icons.addAll([
          const SizedBox(width: 4),
          Tooltip(
            message: label,
            child: Icon(icon, size: 13, color: muted),
          ),
        ]);
      }
    }

    add(model['supports_image'], Icons.image_outlined, '支持图片输入');
    add(model['supports_thinking'], Icons.psychology_outlined, '支持深度思考');
    add(model['supports_web_search'], Icons.travel_explore_outlined, '支持联网搜索');
    add(model['supports_tool_search'], Icons.build_outlined, '支持工具检索');
    add(model['supports_function_tools'], Icons.functions_outlined, '支持函数调用');
    return icons;
  }

  String _ratioText(Object? value) {
    if (value == null) return '-';
    final text = '$value';
    if (text.isEmpty) return '-';
    return double.tryParse(text) != null ? '${text}x' : text;
  }

  Widget _providerIdRow(ProviderModel provider) => Row(
    children: [
      const Icon(Icons.tag, size: 18, color: muted),
      const SizedBox(width: 10),
      const Text('服务商 ID', style: TextStyle(color: muted)),
      const Spacer(),
      Flexible(
        child: Text(
          provider.id,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          textAlign: TextAlign.right,
          style: const TextStyle(fontWeight: FontWeight.w600),
        ),
      ),
    ],
  );

  /// Read-only credential status with an optional clear button. The secret
  /// itself is never shown or editable; it is set only when adding a provider.
  Widget _credentialStatusRow({
    required String label,
    required bool configured,
    required String clearLabel,
    required String clearTitle,
    required List<String> clearArgs,
  }) => Row(
    children: [
      const Icon(Icons.key_outlined, size: 18, color: muted),
      const SizedBox(width: 10),
      Expanded(
        child: Text(
          '$label：${configured ? '已配置' : '尚未配置'}',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
      ),
      if (configured)
        OutlinedButton(
          onPressed: _busy
              ? null
              : () => _clearCredential(clearTitle, clearArgs),
          child: Text(clearLabel),
        ),
    ],
  );

  Widget _connectionFields(ProviderModel provider) {
    if (provider.official) {
      return const Text('此供应商由 Codex 官方 OAuth 登录管理，不能在这里修改连接参数。');
    }
    if (provider.isAwsBedrock) {
      return Column(
        children: [
          _providerIdRow(provider),
          const SizedBox(height: 10),
          labeledField(
            _awsRegionController,
            'AWS Region',
            Icons.public,
            onSubmitted: (_) => _saveProvider(),
          ),
          const SizedBox(height: 10),
          _credentialStatusRow(
            label: 'AWS 凭据',
            configured: provider.awsSigv4Configured,
            clearLabel: '清除凭据',
            clearTitle: '清除 AWS 凭据',
            clearArgs: [
              'provider',
              'update',
              provider.id,
              '--clear-aws-credentials',
            ],
          ),
        ],
      );
    }
    if (!provider.isCustom) {
      return Column(
        children: [
          _providerIdRow(provider),
          const SizedBox(height: 10),
          _credentialStatusRow(
            label: 'API Key',
            configured: provider.apiKeyConfigured,
            clearLabel: '清除密钥',
            clearTitle: '清除密钥',
            clearArgs: ['provider', 'update', provider.id, '--clear-key'],
          ),
        ],
      );
    }
    return Column(
      children: [
        _providerIdRow(provider),
        const SizedBox(height: 10),
        labeledField(
          _displayController,
          '站点名称',
          Icons.title,
          onSubmitted: (_) => _saveProvider(),
        ),
        const SizedBox(height: 10),
        labeledField(
          _baseController,
          'API 地址',
          Icons.link,
          onSubmitted: (_) => _saveProvider(),
        ),
        const SizedBox(height: 10),
        mixinDropdown<String>(
          selected: _protocol,
          decoration: const InputDecoration(
            labelText: 'API 端点',
            prefixIcon: Icon(Icons.alt_route),
          ),
          items: const [
            DropdownMenuItem(
              value: 'open_ai_responses',
              child: Text('Responses'),
            ),
            DropdownMenuItem(
              value: 'open_ai_chat',
              child: Text('Chat Completions'),
            ),
            DropdownMenuItem(
              value: 'anthropic_messages',
              child: Text('Anthropic Messages'),
            ),
          ],
          onChanged: _busy
              ? null
              : (value) {
                  setState(() => _protocol = value ?? _protocol);
                  _saveProvider();
                },
        ),
        const SizedBox(height: 10),
        labeledField(
          _websiteController,
          '官网地址（可选）',
          Icons.public,
          onSubmitted: (_) => _saveProvider(),
        ),
        const SizedBox(height: 10),
        labeledField(
          _imageController,
          '绘图接口路径（可选）',
          Icons.image_outlined,
          hint: '/v1/images/generations',
          onSubmitted: (_) => _saveProvider(),
        ),
        const SizedBox(height: 10),
        _credentialStatusRow(
          label: 'API Key',
          configured: provider.apiKeyConfigured,
          clearLabel: '清除密钥',
          clearTitle: '清除密钥',
          clearArgs: ['provider', 'update', provider.id, '--clear-key'],
        ),
      ],
    );
  }
}

MaterialApp settingsApp({
  required MixinController controller,
  bool autoRefresh = true,
  bool closeHides = true,
}) {
  return MaterialApp(
    debugShowCheckedModeBanner: false,
    title: '供应商设置',
    theme: mixinTheme(),
    locale: mixinLocale,
    supportedLocales: mixinSupportedLocales,
    localizationsDelegates: mixinLocalizationsDelegates,
    home: SettingsPage(
      controller: controller,
      autoRefresh: autoRefresh,
      closeHides: closeHides,
    ),
  );
}
