import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'cli.dart';
import 'controller.dart';
import 'theme.dart';
import 'widgets.dart';

class FusionOption {
  final String id;
  final String displayName;
  final bool available;
  const FusionOption(this.id, this.displayName, {this.available = true});
}

class FusionPage extends StatefulWidget {
  final MixinController controller;
  const FusionPage({super.key, required this.controller});

  @override
  State<FusionPage> createState() => _FusionPageState();
}

class _FusionPageState extends State<FusionPage> {
  final _profileId = TextEditingController(text: 'default');
  final _minSuccessful = TextEditingController(text: '1');
  final _timeoutMs = TextEditingController(text: '300000');
  final Set<String> _panels = {};
  String _judge = '';
  String _final = '';
  bool _showIntermediate = true;
  bool _panelTools = true;
  bool _busy = false;
  String _status = '正在读取配置...';
  List<FusionOption> _options = const [];
  String _loadedId = 'default';

  MixinController get _controller => widget.controller;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _reload());
  }

  @override
  void dispose() {
    _profileId.dispose();
    _minSuccessful.dispose();
    _timeoutMs.dispose();
    super.dispose();
  }

  Future<void> _reload() async {
    setState(() {
      _busy = true;
      _status = '正在读取 Fusion 配置...';
    });
    final profileResult = await _controller.cli.run([
      'fusion',
      'get',
      '--json',
    ]);
    final modelsResult = await _controller.cli.run(['models', '--json']);
    if (!mounted) return;
    // Distinguish a real read failure from "no models"; otherwise a CLI/config
    // error would be silently shown as an empty model list.
    if (!modelsResult.ok || !profileResult.ok) {
      final failed = !modelsResult.ok ? modelsResult : profileResult;
      setState(() {
        _busy = false;
        _status = '读取 Fusion 配置失败：${failed.output}';
      });
      return;
    }
    final envelope = decodeCliObject(profileResult.stdout) ?? const {};
    final profile = envelope['profile'];
    final models = decodeCliJson(modelsResult.stdout);
    final options = <FusionOption>[];
    if (models is List) {
      for (final raw in models.whereType<Map>()) {
        final id = '${raw['id'] ?? ''}';
        if (id.isEmpty || id.startsWith('mixin/fusion/')) continue;
        options.add(FusionOption(id, '${raw['display_name'] ?? id}'));
      }
    }
    options.addAll(_officialOptions());
    options.sort((a, b) => a.displayName.compareTo(b.displayName));
    final storedPanels = profile is Map
        ? ((profile['panel_models'] as List?) ?? const [])
              .map((value) => '$value')
              .toList()
        : <String>[];
    final available = options.map((option) => option.id).toList();
    final panels = storedPanels.where(available.contains).take(8).toList();
    final selected = panels.isEmpty ? available.take(3).toList() : panels;
    final storedJudge = profile is Map ? '${profile['judge_model'] ?? ''}' : '';
    final storedFinal = profile is Map ? '${profile['final_model'] ?? ''}' : '';
    setState(() {
      _options = options;
      _loadedId = profile is Map ? '${profile['id'] ?? 'default'}' : 'default';
      _profileId.text = _loadedId;
      _panels
        ..clear()
        ..addAll(selected);
      _judge = available.contains(storedJudge)
          ? storedJudge
          : (selected.firstOrNull ?? '');
      _final = available.contains(storedFinal)
          ? storedFinal
          : (selected.length > 1 ? selected[1] : _judge);
      _minSuccessful.text = profile is Map
          ? '${profile['min_successful'] ?? 1}'
          : '1';
      _timeoutMs.text = profile is Map
          ? '${profile['timeout_ms'] ?? 300000}'
          : '300000';
      _showIntermediate = profile is Map
          ? profile['show_intermediate_results'] != false
          : true;
      final tools = profile is Map ? profile['panel_tools'] : null;
      _panelTools = tools is Map ? tools['enabled'] != false : true;
      _busy = false;
      _status = options.isEmpty ? '还没有可用模型' : '已加载 Fusion 配置';
    });
  }

  List<FusionOption> _officialOptions() {
    final cache = File(
      '${Platform.environment['USERPROFILE'] ?? '.'}${Platform.pathSeparator}.codex${Platform.pathSeparator}models_cache.json',
    );
    if (!cache.existsSync()) return const [];
    final decoded = decodeCliJson(cache.readAsStringSync());
    if (decoded is! Map) return const [];
    final models = decoded['models'];
    if (models is! List) return const [];
    return models
        .whereType<Map>()
        .map((model) {
          final slug = '${model['slug'] ?? ''}';
          final hidden = '${model['visibility'] ?? 'list'}' == 'hide';
          if (slug.isEmpty || hidden) return null;
          return FusionOption(
            'official:$slug',
            '${model['display_name'] ?? slug} · OpenAI 官方',
          );
        })
        .whereType<FusionOption>()
        .toList();
  }

  String? get _validationError {
    final id = _profileId.text.trim();
    if (id.isEmpty || id.contains('/')) return 'Profile ID 不能为空且不能包含 /';
    if (_panels.isEmpty || _panels.length > 8) return '请选择 1 到 8 个 Panel 模型。';
    if (_judge.isEmpty || _final.isEmpty) return 'Judge 和 Final 模型不能为空。';
    final minSuccessful = int.tryParse(_minSuccessful.text.trim());
    if (minSuccessful == null ||
        minSuccessful < 1 ||
        minSuccessful > _panels.length) {
      return '最少成功 Panel 必须在 1 和 Panel 数量之间。';
    }
    final timeout = int.tryParse(_timeoutMs.text.trim());
    if (timeout == null || timeout <= 0) return '超时必须大于 0。';
    return null;
  }

  Future<void> _save() async {
    final error = _validationError;
    if (error != null) {
      setState(() => _status = error);
      return;
    }
    setState(() {
      _busy = true;
      _status = '正在保存 Fusion 配置...';
    });
    final payload = jsonEncode({
      'id': _profileId.text.trim(),
      'panel_models': _panels.toList(),
      'judge_model': _judge,
      'final_model': _final,
      'min_successful': int.parse(_minSuccessful.text.trim()),
      'max_completion_tokens': 2048,
      'timeout_ms': int.parse(_timeoutMs.text.trim()),
      'show_intermediate_results': _showIntermediate,
      'panel_tools': {
        'enabled': _panelTools,
        'max_rounds': 16,
        'max_calls_per_model': 64,
      },
    });
    final result = await _controller.cli.run([
      'fusion',
      'set',
      '--profile-json',
      payload,
      '--replace-id',
      _loadedId,
    ]);
    if (result.ok) {
      await _controller.cli.run(['service', 'restart']);
      await _controller.cli.run(['refresh-codex-catalog']);
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _status = result.ok ? 'Fusion 已保存并重启网关' : '保存失败：${result.output}';
      if (result.ok) _loadedId = _profileId.text.trim();
    });
  }

  Future<void> _disable() async {
    setState(() {
      _busy = true;
      _status = '正在关闭 Fusion...';
    });
    final result = await _controller.cli.run([
      'fusion',
      'delete',
      '--id',
      _loadedId,
    ]);
    if (result.ok) {
      await _controller.cli.run(['service', 'restart']);
      await _controller.cli.run(['refresh-codex-catalog']);
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _status = result.ok ? 'Fusion 已关闭' : '关闭失败：${result.output}';
      if (result.ok) {
        _panels.clear();
        _judge = '';
        _final = '';
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Column(
        children: [
          GestureDetector(
            onPanStart: (_) => windowManager.startDragging(),
            child: MixinTitleBar(
              title: 'Fusion 设置',
              status: _status,
              onClose: windowManager.close,
            ),
          ),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.all(18),
              children: [
                labeledField(_profileId, 'Profile ID', Icons.badge_outlined),
                const SizedBox(height: 18),
                sectionTitle('Panel 模型（多选）'),
                const SizedBox(height: 8),
                if (_options.isEmpty)
                  const Text('还没有可用模型', style: TextStyle(color: muted))
                else
                  sectionBox(
                    Column(
                      children: [
                        for (
                          var index = 0;
                          index < _options.length;
                          index++
                        ) ...[
                          if (index > 0) const Divider(height: 1),
                          _panelRow(_options[index]),
                        ],
                      ],
                    ),
                  ),
                const SizedBox(height: 18),
                mixinDropdown<String>(
                  selected: _options.any((option) => option.id == _judge)
                      ? _judge
                      : null,
                  decoration: const InputDecoration(
                    labelText: 'Judge 模型',
                    isDense: true,
                  ),
                  items: _options
                      .map(
                        (option) => DropdownMenuItem(
                          value: option.id,
                          child: Text(
                            option.id,
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                      )
                      .toList(),
                  onChanged: _busy
                      ? null
                      : (value) => setState(() => _judge = value ?? _judge),
                ),
                const SizedBox(height: 16),
                mixinDropdown<String>(
                  selected: _options.any((option) => option.id == _final)
                      ? _final
                      : null,
                  decoration: const InputDecoration(
                    labelText: 'Final 模型',
                    isDense: true,
                  ),
                  items: _options
                      .map(
                        (option) => DropdownMenuItem(
                          value: option.id,
                          child: Text(
                            option.id,
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                      )
                      .toList(),
                  onChanged: _busy
                      ? null
                      : (value) => setState(() => _final = value ?? _final),
                ),
                const SizedBox(height: 16),
                labeledField(_minSuccessful, '最少成功 Panel', Icons.filter_1),
                const SizedBox(height: 16),
                labeledField(_timeoutMs, '单模型超时 (ms)', Icons.timer_outlined),
                const SizedBox(height: 16),
                labeledSwitch(
                  title: '在回答中显示 Panel / Judge 中间结果',
                  value: _showIntermediate,
                  onChanged: _busy
                      ? null
                      : (value) => setState(() => _showIntermediate = value),
                ),
                labeledSwitch(
                  title: '允许 Panel 使用进程内只读工具',
                  value: _panelTools,
                  onChanged: _busy
                      ? null
                      : (value) => setState(() => _panelTools = value),
                ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.all(16),
            child: Row(
              children: [
                if (_busy)
                  const SizedBox(
                    width: 18,
                    height: 18,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  ),
                const Spacer(),
                OutlinedButton(
                  onPressed: _busy ? null : _disable,
                  child: const Text('关闭 Fusion'),
                ),
                const SizedBox(width: 8),
                FilledButton(
                  onPressed: _busy ? null : _save,
                  child: const Text('保存并重启网关'),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _panelRow(FusionOption option) {
    final selected = _panels.contains(option.id);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  option.id,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontWeight: FontWeight.w600),
                ),
                Text(
                  option.displayName,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(color: muted, fontSize: 12),
                ),
              ],
            ),
          ),
          Checkbox(
            value: selected,
            onChanged: _busy
                ? null
                : (checked) => setState(() {
                    if (checked == true) {
                      if (_panels.length < 8) _panels.add(option.id);
                    } else {
                      _panels.remove(option.id);
                    }
                  }),
          ),
        ],
      ),
    );
  }
}

MaterialApp fusionApp({required MixinController controller}) {
  return MaterialApp(
    debugShowCheckedModeBanner: false,
    title: 'Fusion 设置',
    theme: mixinTheme(),
    locale: mixinLocale,
    supportedLocales: mixinSupportedLocales,
    localizationsDelegates: mixinLocalizationsDelegates,
    home: FusionPage(controller: controller),
  );
}
