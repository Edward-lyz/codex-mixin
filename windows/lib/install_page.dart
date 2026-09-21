import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'controller.dart';
import 'theme.dart';
import 'widgets.dart';

/// One selectable integration mode for an install target.
class InstallMode {
  final String value;
  final IconData icon;
  final String title;
  final String description;
  const InstallMode({
    required this.value,
    required this.icon,
    required this.title,
    required this.description,
  });
}

/// Describes how to install Codex Mixin into one downstream tool. Register new
/// platforms in [installTargetFor] so this window can be reused as-is.
class InstallTarget {
  final String id;
  final String name;
  final bool supported;
  final List<InstallMode> modes;
  final List<(String, String)> locations;
  final List<String> Function(String mode) buildArgs;
  const InstallTarget({
    required this.id,
    required this.name,
    this.supported = true,
    this.modes = const [],
    this.locations = const [],
    required this.buildArgs,
  });
}

List<String> _codexArgs(String mode) => [
  'install-codex',
  mode == 'official' ? '--codex-oauth-proxy' : '--custom-only',
];

const _codexTarget = InstallTarget(
  id: 'codex',
  name: 'Codex',
  modes: [
    InstallMode(
      value: 'official',
      icon: Icons.verified_user_outlined,
      title: '官方账号模式（推荐：已登录 Codex）',
      description:
          '前提是已在官方 Codex App 登录并打开过一次。保留官方认证、GPT、插件、云任务和账户功能；'
          '自定义模型经本地网关加入同一个模型选择器。',
    ),
    InstallMode(
      value: 'custom',
      icon: Icons.tune_outlined,
      title: '仅自定义模型模式（没有官方登录也可用）',
      description:
          '用本地 Bedrock 形态的占位身份开启模型选择器。请求只到本地网关，不连接 AWS；'
          '官方插件、云任务和账户功能不可用。',
    ),
  ],
  locations: [
    ('Codex 配置', '~/.codex/config.toml'),
    ('模型目录', '~/.codex/model-catalogs/mixin-models.json'),
  ],
  buildArgs: _codexArgs,
);

/// Resolve the target descriptor for a platform id. Unsupported platforms still
/// return a descriptor so the window can open with a uniform placeholder.
InstallTarget installTargetFor(String id) {
  switch (id) {
    case 'codex':
      return _codexTarget;
    case 'claude':
      return InstallTarget(
        id: id,
        name: 'Claude Code',
        supported: false,
        buildArgs: (_) => const <String>[],
      );
    case 'dsh':
      return InstallTarget(
        id: id,
        name: 'DSH',
        supported: false,
        buildArgs: (_) => const <String>[],
      );
    case 'opencode':
      return InstallTarget(
        id: id,
        name: 'OpenCode',
        supported: false,
        buildArgs: (_) => const <String>[],
      );
    case 'pi':
      return InstallTarget(
        id: id,
        name: 'Pi',
        supported: false,
        buildArgs: (_) => const <String>[],
      );
    default:
      return InstallTarget(
        id: id,
        name: id,
        supported: false,
        buildArgs: (_) => const <String>[],
      );
  }
}

class InstallPage extends StatefulWidget {
  final MixinController controller;
  final String targetId;
  const InstallPage({
    super.key,
    required this.controller,
    required this.targetId,
  });

  @override
  State<InstallPage> createState() => _InstallPageState();
}

class _InstallPageState extends State<InstallPage> {
  String? _mode;
  bool _busy = false;

  InstallTarget get _target => installTargetFor(widget.targetId);

  Future<void> _install() async {
    final mode = _mode;
    if (mode == null || _busy) return;
    setState(() => _busy = true);
    final title = '安装到 ${_target.name}';
    final result = await runWithProgress(
      context,
      title: title,
      resultText: (r) => r.ok
          ? '$title成功'
          : '$title失败：${formatCliReport(r.output).trim().isEmpty ? '未返回具体原因' : formatCliReport(r.output).trim()}',
      action: (onProgress) => widget.controller.runAction(
        title,
        _target.buildArgs(mode),
        onProgress: onProgress,
      ),
    );
    if (!mounted) return;
    setState(() => _busy = false);
    // On success the config is applied; close the window. On failure keep it
    // open so the user can adjust the mode and retry.
    if (result.ok) await windowManager.close();
  }

  @override
  Widget build(BuildContext context) {
    final target = _target;
    return Scaffold(
      body: Column(
        children: [
          GestureDetector(
            onPanStart: (_) => windowManager.startDragging(),
            child: MixinTitleBar(
              title: '安装到 ${target.name}',
              onClose: windowManager.close,
            ),
          ),
          Expanded(child: target.supported ? _form(target) : _unsupported()),
          if (target.supported) _footer(),
        ],
      ),
    );
  }

  Widget _unsupported() => const Center(
    child: Padding(
      padding: EdgeInsets.all(24),
      child: Text(
        '该平台暂不支持，敬请期待。',
        style: TextStyle(color: muted, fontSize: 14),
      ),
    ),
  );

  Widget _form(InstallTarget target) => ListView(
    padding: const EdgeInsets.fromLTRB(20, 10, 20, 10),
    children: [
      const Text(
        '选择集成模式',
        style: TextStyle(fontWeight: FontWeight.w600, fontSize: 13),
      ),
      const SizedBox(height: 12),
      for (final mode in target.modes) _modeTile(mode),
      const SizedBox(height: 6),
      _locationBox(target),
    ],
  );

  Widget _footer() => Container(
    padding: const EdgeInsets.fromLTRB(20, 10, 20, 14),
    decoration: const BoxDecoration(
      border: Border(top: BorderSide(color: line)),
    ),
    child: Row(
      mainAxisAlignment: MainAxisAlignment.end,
      children: [
        TextButton(
          onPressed: _busy ? null : windowManager.close,
          child: const Text('取消'),
        ),
        const SizedBox(width: 8),
        FilledButton(
          onPressed: (_mode == null || _busy) ? null : _install,
          child: const Text('安装'),
        ),
      ],
    ),
  );

  Widget _modeTile(InstallMode mode) {
    final selected = _mode == mode.value;
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: GestureDetector(
        onTap: () => setState(() => _mode = mode.value),
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          padding: const EdgeInsets.all(14),
          decoration: BoxDecoration(
            color: selected ? const Color(0xffeef4ff) : const Color(0xfffafbfc),
            border: Border.all(
              color: selected ? accent : line,
              width: selected ? 1.5 : 1,
            ),
            borderRadius: BorderRadius.circular(12),
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _radioDot(selected),
              const SizedBox(width: 12),
              Expanded(child: _modeText(mode, selected)),
            ],
          ),
        ),
      ),
    );
  }

  Widget _modeText(InstallMode mode, bool selected) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Row(
        children: [
          Icon(
            mode.icon,
            size: 17,
            color: selected ? accent : const Color(0xff3f4247),
          ),
          const SizedBox(width: 7),
          Expanded(
            child: Text(
              mode.title,
              style: TextStyle(
                fontSize: 14,
                height: 1.3,
                fontWeight: FontWeight.w600,
                color: selected ? accent : const Color(0xff1d2024),
              ),
            ),
          ),
        ],
      ),
      const SizedBox(height: 6),
      Text(
        mode.description,
        style: const TextStyle(color: muted, fontSize: 12.5, height: 1.5),
      ),
    ],
  );

  Widget _radioDot(bool selected) => Container(
    margin: const EdgeInsets.only(top: 2),
    width: 18,
    height: 18,
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
              width: 8,
              height: 8,
              decoration: const BoxDecoration(
                shape: BoxShape.circle,
                color: accent,
              ),
            ),
          )
        : null,
  );

  Widget _locationBox(InstallTarget target) {
    if (target.locations.isEmpty) return const SizedBox.shrink();
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
      decoration: BoxDecoration(
        color: const Color(0xfff6f7f9),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Row(
            children: [
              Icon(Icons.folder_outlined, size: 15, color: muted),
              SizedBox(width: 6),
              Text(
                '安装位置',
                style: TextStyle(fontWeight: FontWeight.w600, fontSize: 13),
              ),
            ],
          ),
          for (final loc in target.locations) _locationRow(loc.$1, loc.$2),
        ],
      ),
    );
  }

  Widget _locationRow(String label, String value) => Padding(
    padding: const EdgeInsets.only(top: 6),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 64,
          child: Text(
            label,
            style: const TextStyle(color: muted, fontSize: 12),
          ),
        ),
        Expanded(
          child: SelectableText(
            value,
            style: const TextStyle(
              fontSize: 12,
              height: 1.4,
              color: Color(0xff3f4247),
            ),
          ),
        ),
      ],
    ),
  );
}

MaterialApp installApp({
  required MixinController controller,
  required String target,
}) {
  return MaterialApp(
    debugShowCheckedModeBanner: false,
    title: '安装配置',
    theme: mixinTheme(),
    locale: mixinLocale,
    supportedLocales: mixinSupportedLocales,
    localizationsDelegates: mixinLocalizationsDelegates,
    home: InstallPage(controller: controller, targetId: target),
  );
}
