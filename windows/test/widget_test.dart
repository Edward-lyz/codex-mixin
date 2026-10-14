import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:codex_mixin_ui/agent_models.dart';
import 'package:codex_mixin_ui/agents_page.dart';
import 'package:codex_mixin_ui/controller.dart';
import 'package:codex_mixin_ui/cli.dart';
import 'package:codex_mixin_ui/config_backups.dart';
import 'package:codex_mixin_ui/main.dart';
import 'package:codex_mixin_ui/models.dart';
import 'package:codex_mixin_ui/widgets.dart';

class _RecordingCli extends MixinCli {
  final events = <String>[];
  final calls = <List<String>>[];

  @override
  Future<CliResult> run(
    List<String> args, {
    Map<String, String>? secrets,
    void Function(String line)? onProgress,
  }) async {
    final label = args.first;
    calls.add(args);
    events.add('start:$label');
    await Future<void>.delayed(Duration.zero);
    events.add('end:$label');
    return const CliResult(0, 'ok', '');
  }
}

/// Fake `agent` CLI: serves the JSON contract and records every call.
class _AgentCli extends MixinCli {
  _AgentCli({
    required this.listJson,
    String? versionsJson,
    String? upgradeAllJson,
    this.versionsResult,
    this.upgradeResult,
  }) : versionsJson = versionsJson ?? '{"agents":[]}',
       upgradeAllJson = upgradeAllJson ?? '{"results":[]}';

  final calls = <List<String>>[];
  final String listJson;
  final String versionsJson;
  final String upgradeAllJson;
  final CliResult? versionsResult;
  final CliResult? upgradeResult;

  @override
  Future<CliResult> run(
    List<String> args, {
    Map<String, String>? secrets,
    void Function(String line)? onProgress,
  }) async {
    calls.add(args);
    await Future<void>.delayed(Duration.zero);
    if (args.length >= 2 && args[0] == 'agent' && args[1] == 'list') {
      return CliResult(0, listJson, '');
    }
    if (args.length >= 2 && args[0] == 'agent' && args[1] == 'versions') {
      return versionsResult ?? CliResult(0, versionsJson, '');
    }
    if (args.length >= 3 &&
        args[0] == 'agent' &&
        args[1] == 'upgrade' &&
        args[2] == '--all') {
      return CliResult(0, upgradeAllJson, '');
    }
    if (args.length >= 3 && args[0] == 'agent' && args[1] == 'upgrade') {
      return upgradeResult ??
          const CliResult(0, '{"id":"claude","cli":{"version":"1.3.0"}}', '');
    }
    return const CliResult(0, '{}', '');
  }
}

/// Six-agent list contract fixture. `cli` is null for Claude Code so the async
/// `agent versions` pass is what fills its version in.
String _agentListJson({String codexState = 'managed'}) => jsonEncode({
  'agents': [
    {
      'id': 'codex',
      'name': 'Codex',
      'detected': true,
      'integration': {
        'state': codexState,
        'mode': codexState == 'managed' ? 'custom-only' : null,
        'detail': '托管配置已写入 ~/.codex/config.toml',
        'error': null,
      },
      'cli': {
        'version': '0.9.0',
        'latest': '0.9.0',
        'via': 'npm',
        'path': r'C:\Users\me\AppData\Roaming\npm\codex.cmd',
        'command': 'npm install -g @openai/codex',
        'update': false,
        'error': null,
      },
    },
    {
      'id': 'claude',
      'name': 'Claude Code',
      'detected': true,
      'integration': {
        'state': 'unmanaged',
        'mode': null,
        'detail': '',
        'error': null,
      },
      'cli': null,
    },
    {
      'id': 'claude-desktop',
      'name': 'Claude Desktop',
      'detected': true,
      'integration': {
        'state': 'error',
        'mode': null,
        'detail': '',
        'error': '配置目录不可写',
      },
      'cli': {
        'version': null,
        'latest': null,
        'via': 'app',
        'path': r'C:\Program Files\Claude\Claude.exe',
        'command': null,
        'app_version': '1.5.0',
        'update': false,
        'error': '无法读取 App 版本',
      },
    },
    {
      'id': 'dsh',
      'name': 'DSH',
      'detected': false,
      'integration': {'state': 'unmanaged'},
      'cli': null,
    },
    {
      'id': 'opencode',
      'name': 'OpenCode',
      'detected': false,
      'integration': {'state': 'unmanaged'},
      'cli': null,
    },
    {
      'id': 'pi',
      'name': 'Pi',
      'detected': false,
      'integration': {'state': 'unmanaged'},
      'cli': null,
    },
  ],
});

String _agentVersionsJson() => jsonEncode({
  'agents': [
    {
      'id': 'codex',
      'cli': {
        'version': '0.9.0',
        'latest': '0.9.1',
        'via': 'npm',
        'path': 'C:/npm/codex.cmd',
        'command': 'npm install -g @openai/codex',
        'update': true,
        'error': null,
      },
    },
    {
      'id': 'claude',
      'cli': {
        'version': '1.2.0',
        'latest': '1.3.0',
        'via': 'npm',
        'path': 'C:/npm/claude.cmd',
        'command': 'npm install -g @anthropic-ai/claude-code',
        'update': true,
        'error': null,
      },
    },
    {
      'id': 'claude-desktop',
      'cli': {
        'version': null,
        'latest': null,
        'via': 'app',
        'path': 'C:/Claude/Claude.exe',
        'command': null,
        'app_version': '1.5.0',
        'update': false,
        'error': '无法读取 App 版本',
      },
    },
  ],
});

bool _called(List<List<String>> calls, String expected) =>
    calls.any((call) => call.join(' ') == expected);

/// Pump timers/microtasks without requiring animations to settle: the page
/// shows a progress spinner while the fake CLI is in flight.
Future<void> _pump(WidgetTester tester) async {
  // 400 ms also covers the dialog route transitions before a tap lands.
  for (var i = 0; i < 8; i++) {
    await tester.pump(const Duration(milliseconds: 50));
  }
}

void main() {
  test('parses the agent list contract', () {
    final json = decodeCliJson(_agentListJson()) as Map<String, dynamic>;
    final agents = parseAgentRows(json['agents']);
    expect(agents.map((agent) => agent.id), [
      'codex',
      'claude',
      'claude-desktop',
      'dsh',
      'opencode',
      'pi',
    ]);
    expect(agents.first.integration.stateLabel, '已接入');
    expect(agents.first.integration.modeLabel, '仅自定义模型模式');
    expect(agents[1].hasUpdate, isFalse);
    // Claude Desktop reports an app version instead of a CLI version.
    expect(agents[2].cli?.displayVersion, '1.5.0');
    expect(agents[2].integration.error, '配置目录不可写');
    expect(agents[3].detected, isFalse);
  });

  test('a Windows startup-app disable is respected', () {
    const enabled =
        '    CodexMixin    REG_BINARY    0200000000000000000000000000000000';
    const disabled =
        '    CodexMixin    REG_BINARY    0300000000000000000000000000000000';
    expect(startupEntryDisabled(enabled), isFalse);
    expect(startupEntryDisabled(disabled), isTrue);
    expect(
      startupEntryDisabled(
        'ERROR: The system was unable to find the specified registry key '
        'or value.',
      ),
      isFalse,
    );
  });

  test('backup dialog script keeps the WinForms type on one line', () {
    final script = buildBackupDialogScript(
      'SaveFileDialog',
      r"$dialog.Title = 'Export'",
    );
    expect(
      script,
      contains(r'$dialog = New-Object System.Windows.Forms.SaveFileDialog'),
    );
    expect(script, isNot(contains('System.Windows.Forms.\nSaveFileDialog')));
  });

  test('decodes pretty-printed CLI JSON', () {
    final value =
        decodeCliJson('''
{
  "providers": [
    {"id": "baidu-oneapi", "display_name": "Baidu OneAPI"}
  ]
}
''')
            as Map<String, dynamic>;
    expect((value['providers'] as List).single['id'], 'baidu-oneapi');
  });

  test('formats actionable CLI errors for the Windows UI', () {
    expect(
      formatCliReport('no enabled Baidu reporting provider is configured'),
      contains('请在供应商设置中开启'),
    );
    expect(
      formatCliReport('Codex config is not managed by codex-mixin'),
      contains('Agent 接入管理'),
    );
  });

  test('formats streamed CLI progress in Chinese', () {
    expect(
      formatCliProgress(
        'MIXIN_PROGRESS Writing Codex config and model catalog',
      ),
      '正在写入 Codex 配置与模型目录',
    );
    expect(
      formatCliProgress('MIXIN_PROGRESS Downloading DUCX 64/132 MiB'),
      '正在下载 DUCX：64 / 132 MiB',
    );
  });

  test(
    'serializes controller actions instead of returning a busy failure',
    () async {
      final cli = _RecordingCli();
      final controller = MixinController(cli: cli);
      final first = controller.runAction('first', ['first']);
      final second = controller.runAction('second', ['second']);

      final results = await Future.wait([first, second]);

      expect(results.every((result) => result.ok), isTrue);
      expect(cli.events, [
        'start:first',
        'end:first',
        'start:second',
        'end:second',
      ]);
    },
  );

  testWidgets('renders the Codex Mixin dashboard', (WidgetTester tester) async {
    await tester.binding.setSurfaceSize(const Size(1200, 800));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(const CodexMixinApp(autoRefresh: false));
    await tester.pump();
    expect(find.text('供应商设置'), findsOneWidget);
    expect(find.text('还没有 Provider'), findsOneWidget);
    expect(find.text('新增自定义 Provider 后可排序'), findsNothing);
    expect(find.byKey(const Key('toggle-provider-sidebar')), findsOneWidget);
    expect(find.byIcon(Icons.chevron_left_rounded), findsOneWidget);
  });

  testWidgets('provider settings expose models and connection tabs', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1200, 800));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final controller = MixinController();
    controller.snapshot = GatewaySnapshot(
      providers: [
        ProviderModel.fromJson({
          'id': 'baidu-oneapi',
          'display_name': 'Baidu OneAPI',
          'kind': 'configured',
          'icon': 'baidu',
          'enabled': true,
          'preset_id': 'baidu-oneapi',
          'protocol': 'open_ai_responses',
          'base_url': 'https://example.invalid',
          'selected_models': ['ernie-4.5'],
          'cached_models': [
            {'id': 'ernie-4.5', 'display_name': 'ERNIE 4.5'},
            {'id': 'ernie-x1', 'display_name': 'ERNIE X1'},
          ],
          'readiness': 'healthy',
        }),
      ],
      gatewayRunning: false,
      serviceTitle: '本地网关已停止',
      serviceEndpoint: 'http://127.0.0.1:64088/v1',
      quotaRows: const [],
      usageRows: const [],
      status: '配置已同步',
    );
    await tester.pumpWidget(
      settingsApp(
        controller: controller,
        autoRefresh: false,
        closeHides: false,
      ),
    );
    await tester.pump();

    expect(find.text('连接设置'), findsOneWidget);
    expect(find.text('模型'), findsOneWidget);
    expect(find.text('连接配置'), findsOneWidget);

    await tester.tap(find.text('模型'));
    await tester.pumpAndSettle();
    expect(find.text('刷新模型'), findsOneWidget);
    expect(find.text('保存模型设置'), findsOneWidget);
    expect(find.text('测速'), findsOneWidget);
    expect(find.text('ernie-4.5'), findsOneWidget);
    expect(find.text('ernie-x1'), findsOneWidget);

    final checkbox = tester.widget<Checkbox>(find.byType(Checkbox).at(0));
    expect(checkbox.value, isTrue);
    await tester.tap(find.byType(Checkbox).at(0));
    await tester.pump();
    expect(tester.widget<Checkbox>(find.byType(Checkbox).at(0)).value, isFalse);
  });

  testWidgets('saving skips a delisted model context override', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1200, 800));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final cli = _RecordingCli();
    final controller = MixinController(cli: cli);
    controller.snapshot = GatewaySnapshot(
      providers: [
        ProviderModel.fromJson({
          'id': 'custom',
          'display_name': 'Custom',
          'kind': 'configured',
          'selected_models': ['available'],
          'cached_models': [
            {'id': 'available'},
          ],
          'model_context_overrides': {'delisted': 300000},
        }),
      ],
      gatewayRunning: false,
      serviceTitle: '本地网关已停止',
      serviceEndpoint: 'http://127.0.0.1:64088/v1',
      quotaRows: const [],
      usageRows: const [],
      status: '配置已同步',
    );
    await tester.pumpWidget(
      settingsApp(controller: controller, autoRefresh: false, closeHides: false),
    );
    await tester.pump();
    await tester.tap(find.text('模型'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('保存模型设置'));
    await tester.pumpAndSettle();

    final selection = cli.calls.firstWhere(
      (args) => args.length > 2 && args[0] == 'provider' && args[1] == 'select',
    );
    expect(selection, containsAllInOrder(['--clear-model-context', 'delisted']));
    expect(selection, isNot(contains('--model-context')));
  });

  testWidgets('supports compact and large settings layouts', (
    WidgetTester tester,
  ) async {
    for (final size in [const Size(760, 600), const Size(1200, 800)]) {
      await tester.binding.setSurfaceSize(size);
      await tester.pumpWidget(const CodexMixinApp(autoRefresh: false));
      await tester.pump();
      expect(tester.takeException(), isNull, reason: 'layout failed at $size');
    }
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets('tray popup renders the macOS-style dashboard', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(360, 720));
    await tester.pumpWidget(
      const CodexMixinApp(autoRefresh: false, initialTrayPopup: true),
    );
    await tester.pump();

    expect(find.text('供应商设置'), findsNothing);
    expect(find.text('本地网关已停止'), findsOneWidget);
    expect(find.byType(Switch), findsOneWidget);
    expect(find.text('模型与服务...'), findsOneWidget);
    expect(find.text('设置与模型'), findsNothing);
    expect(find.text('刷新状态与额度'), findsNothing);
    expect(find.text('高级'), findsOneWidget);
    expect(find.text('Agent 接入管理...'), findsOneWidget);
    expect(find.text('安装与恢复'), findsNothing);
    expect(find.text('健康检测和修复'), findsNothing);
    expect(find.text('登录时启动并开启服务'), findsNothing);
    expect(find.text('关于'), findsOneWidget);
    expect(find.text('退出 Codex Mixin'), findsOneWidget);
    expect(find.text('退出并恢复原配置'), findsOneWidget);
    expect(tester.takeException(), isNull);

    await tester.binding.setSurfaceSize(null);
  });

  testWidgets('tray usage dashboard renders quota, range and token bars', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(360, 720));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final controller = MixinController();
    controller.snapshot = GatewaySnapshot(
      providers: [
        ProviderModel.fromJson({
          'id': 'deepseek',
          'display_name': 'DeepSeek',
          'kind': 'configured',
          'icon': 'deepseek',
          'enabled': true,
          'preset_id': 'deepseek',
          'selected_models': ['deepseek-flash'],
          'cached_models': [
            {'id': 'deepseek-flash'},
          ],
        }),
      ],
      gatewayRunning: true,
      serviceTitle: '本地网关运行中',
      serviceEndpoint: 'http://127.0.0.1:64088/v1',
      quotaRows: const [
        {
          'provider_id': 'deepseek',
          'label': '额度',
          'remaining': 2.12,
          'currency': 'CNY',
        },
      ],
      usageRows: const [
        {
          'provider_id': 'deepseek',
          'model_id': 'deepseek-flash',
          'request_count': 41,
          'input_tokens': 187300,
          'cache_read_tokens': 2100000,
          'cache_creation_tokens': 0,
          'output_tokens': 20000,
          'cache_hit_percent': 91.9,
          'average_ttft_ms': 1745,
          'output_tps': 256.5,
        },
      ],
      status: '配置已同步',
    );
    await tester.pumpWidget(
      trayApp(controller: controller, windowId: 0, autoRefresh: false),
    );
    await tester.pump();

    expect(find.text('本地网关运行中'), findsOneWidget);
    expect(find.textContaining('余额'), findsOneWidget);
    expect(find.text('1天'), findsOneWidget);
    expect(find.text('1月'), findsOneWidget);
    expect(find.text('deepseek-flash'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('token-bar-deepseek-flash')));
    await tester.pump();
    expect(find.text('请求'), findsOneWidget);
    expect(find.text('每秒吞吐'), findsOneWidget);
    expect(find.text('256.5 tok/s'), findsOneWidget);
  });

  testWidgets('tray usage explains why token data is unavailable', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(360, 720));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final controller = MixinController();
    controller.snapshot = GatewaySnapshot(
      providers: [
        ProviderModel.fromJson({
          'id': 'deepseek',
          'display_name': 'DeepSeek',
          'kind': 'configured',
          'icon': 'deepseek',
          'enabled': true,
        }),
      ],
      gatewayRunning: false,
      serviceTitle: '本地网关已停止',
      serviceEndpoint: 'http://127.0.0.1:64088/v1',
      quotaRows: const [],
      usageRows: const [],
      status: '配置已同步',
    );
    await tester.pumpWidget(
      trayApp(controller: controller, windowId: 0, autoRefresh: false),
    );
    await tester.pump();

    expect(find.text('网关未运行，无法读取 Token 使用'), findsOneWidget);
  });

  testWidgets('tray usage reports query failures separately from empty data', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(360, 720));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final controller = MixinController();
    controller.snapshot = GatewaySnapshot(
      providers: [
        ProviderModel.fromJson({
          'id': 'deepseek',
          'display_name': 'DeepSeek',
          'kind': 'configured',
          'icon': 'deepseek',
          'enabled': true,
        }),
      ],
      gatewayRunning: true,
      serviceTitle: '本地网关运行中',
      serviceEndpoint: 'http://127.0.0.1:64088/v1',
      quotaRows: const [],
      usageRows: const [],
      usageError: 'usage 查询失败',
      status: '配置已同步',
    );
    await tester.pumpWidget(
      trayApp(controller: controller, windowId: 0, autoRefresh: false),
    );
    await tester.pump();

    expect(find.textContaining('Token 使用暂不可用'), findsOneWidget);
    expect(find.textContaining('暂无 Token 使用记录'), findsNothing);
  });

  testWidgets('flyout window opens the agent management window', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(280, 320));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(
      flyoutApp(
        controller: MixinController(),
        windowId: 3,
        title: 'Agent 接入管理',
        trayWindowId: 1,
      ),
    );
    await tester.pump();

    expect(find.text('Agent 接入管理...'), findsOneWidget);
    expect(find.text('安装到 Codex...'), findsNothing);
    expect(find.text('从 Pi 卸载...'), findsNothing);
  });

  testWidgets('flyout window renders settings and advanced actions', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(280, 320));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(
      flyoutApp(
        controller: MixinController(),
        windowId: 4,
        title: '设置与模型',
        trayWindowId: 1,
      ),
    );
    await tester.pump();
    expect(find.text('供应商设置...'), findsOneWidget);
    expect(find.text('模型选择与测速...'), findsNothing);
  });

  testWidgets('provider dialog shows preset-specific credentials', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1200, 800));
    await tester.pumpWidget(const CodexMixinApp(autoRefresh: false));
    await tester.tap(find.byTooltip('新增供应商'));
    await tester.pumpAndSettle();

    expect(find.text('额度用户名'), findsOneWidget);
    expect(find.text('API Key'), findsOneWidget);
    expect(find.text('AWS Region'), findsNothing);

    await tester.tap(find.text('Baidu OneAPI').last);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Amazon Bedrock').last);
    await tester.pumpAndSettle();

    expect(find.text('AWS Region'), findsOneWidget);
    expect(find.text('Access Key ID'), findsOneWidget);
    expect(find.text('Secret Access Key'), findsOneWidget);
    expect(find.text('Session Token（可选）'), findsOneWidget);
    expect(find.text('API Key'), findsNothing);

    await tester.tap(find.text('Amazon Bedrock').last);
    await tester.pumpAndSettle();
    await tester.tap(find.text('OpenCode Go').last);
    await tester.pumpAndSettle();
    expect(find.text('工作区 ID'), findsOneWidget);
    expect(find.text('Auth Cookie'), findsOneWidget);

    await tester.binding.setSurfaceSize(null);
  });

  testWidgets('agents page lists detection, integration state and versions', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(760, 720));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final cli = _AgentCli(
      listJson: _agentListJson(),
      versionsJson: _agentVersionsJson(),
    );
    final controller = MixinController(cli: cli);
    await tester.pumpWidget(
      agentsApp(controller: controller, checkStartupAtLogin: false),
    );
    await _pump(tester);

    expect(find.text('Codex'), findsOneWidget);
    expect(find.text('Claude Code'), findsOneWidget);
    expect(find.text('已接入'), findsOneWidget);
    expect(find.text('未接入'), findsOneWidget);
    expect(find.text('检测错误'), findsOneWidget);
    expect(find.text('仅自定义模型模式'), findsOneWidget);
    // Versions come from the async `agent versions --refresh` pass; the list
    // itself never blocks on them.
    expect(find.text('0.9.0 → 0.9.1'), findsOneWidget);
    expect(find.text('1.2.0 → 1.3.0'), findsOneWidget);
    expect(find.text('1.5.0'), findsOneWidget);
    expect(_called(cli.calls, 'agent list --json'), isTrue);
    expect(_called(cli.calls, 'agent versions --json --refresh'), isTrue);

    // Undetected agents stay folded away until asked for.
    expect(find.text('DSH'), findsNothing);
    expect(find.text('未检测到的 Agent（3）'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('agent-fold-undetected')));
    await _pump(tester);
    expect(find.text('DSH'), findsOneWidget);
    expect(find.text('OpenCode'), findsOneWidget);
    expect(find.text('Pi'), findsOneWidget);
    expect(find.text('未检测到'), findsNWidgets(3));

    // Details are on demand and carry the CLI's own error text.
    await tester.tap(find.byKey(const ValueKey('agent-row-claude-desktop')));
    await _pump(tester);
    expect(find.text('程序检测'), findsOneWidget);
    expect(find.text('配置目录不可写'), findsOneWidget);
    expect(find.text('无法读取 App 版本'), findsOneWidget);
  });

  testWidgets('first Codex connect asks for the account mode', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(760, 720));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final cli = _AgentCli(
      listJson: _agentListJson(codexState: 'unmanaged'),
      versionsJson: _agentVersionsJson(),
    );
    final controller = MixinController(cli: cli);
    await tester.pumpWidget(
      agentsApp(controller: controller, checkStartupAtLogin: false),
    );
    await _pump(tester);

    await tester.tap(find.byKey(const ValueKey('agent-switch-codex')));
    await _pump(tester);
    expect(find.text('官方账号模式（推荐：已登录 Codex）'), findsOneWidget);
    expect(find.text('仅自定义模型模式（没有官方登录也可用）'), findsOneWidget);
    expect(_called(cli.calls, 'agent connect codex'), isFalse);

    await tester.tap(find.byKey(const ValueKey('codex-mode-custom-only')));
    await _pump(tester);
    await tester.tap(find.text('接入'));
    await _pump(tester);

    expect(_called(cli.calls, 'agent connect codex --mode custom-only'), isTrue);
    expect(find.text('已接入 Codex'), findsOneWidget);
    await tester.tap(find.text('确认'));
    await _pump(tester);
  });

  testWidgets('disconnect explains recovery and keeps the shared gateway', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(760, 720));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final cli = _AgentCli(
      listJson: _agentListJson(),
      versionsJson: _agentVersionsJson(),
    );
    final controller = MixinController(cli: cli);
    await tester.pumpWidget(
      agentsApp(controller: controller, checkStartupAtLogin: false),
    );
    await _pump(tester);

    await tester.tap(find.byKey(const ValueKey('agent-switch-codex')));
    await _pump(tester);
    expect(find.text('断开 Codex 接入？'), findsOneWidget);
    expect(find.textContaining('共享网关继续运行'), findsOneWidget);

    await tester.tap(find.text('断开'));
    await _pump(tester);

    expect(_called(cli.calls, 'agent disconnect codex'), isTrue);
    expect(cli.calls.any((call) => call.contains('service')), isFalse);
    expect(find.text('已断开 Codex 接入'), findsOneWidget);
    await tester.tap(find.text('确认'));
    await _pump(tester);
  });

  testWidgets('upgrade failure and version errors are shown per agent', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(760, 720));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final cli = _AgentCli(
      listJson: _agentListJson(),
      versionsJson: _agentVersionsJson(),
      upgradeResult: const CliResult(1, '', 'npm EACCES: permission denied'),
    );
    final controller = MixinController(cli: cli);
    await tester.pumpWidget(
      agentsApp(controller: controller, checkStartupAtLogin: false),
    );
    await _pump(tester);

    await tester.tap(find.byKey(const ValueKey('agent-upgrade-claude')));
    await _pump(tester);
    expect(find.textContaining('npm EACCES'), findsOneWidget);
    await tester.tap(find.text('确认'));
    await _pump(tester);
    expect(_called(cli.calls, 'agent upgrade claude --json'), isTrue);
  });

  testWidgets('a version read failure never hides the agent list', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(760, 720));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final cli = _AgentCli(
      listJson: _agentListJson(),
      versionsResult: const CliResult(1, '', 'npm 不可用'),
    );
    final controller = MixinController(cli: cli);
    await tester.pumpWidget(
      agentsApp(controller: controller, checkStartupAtLogin: false),
    );
    await _pump(tester);

    expect(find.text('Codex'), findsOneWidget);
    expect(find.textContaining('版本信息读取失败'), findsOneWidget);
    expect(find.textContaining('npm 不可用'), findsOneWidget);
  });

  testWidgets('an upgrade refusal from the CLI is not reported as success', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(760, 720));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final cli = _AgentCli(
      listJson: _agentListJson(),
      versionsJson: _agentVersionsJson(),
      upgradeResult: CliResult(
        0,
        jsonEncode({
          'id': 'claude',
          'cli': {
            'version': '1.2.0',
            'latest': '1.3.0',
            'via': 'npm',
            'path': 'C:/npm/claude.cmd',
            'update': true,
            'error': 'cannot tell how claude was installed',
          },
        }),
        '',
      ),
    );
    final controller = MixinController(cli: cli);
    await tester.pumpWidget(
      agentsApp(controller: controller, checkStartupAtLogin: false),
    );
    await _pump(tester);

    await tester.tap(find.byKey(const ValueKey('agent-upgrade-claude')));
    await _pump(tester);
    expect(
      find.textContaining('cannot tell how claude was installed'),
      findsOneWidget,
    );
    expect(find.textContaining('已升级 Claude Code'), findsNothing);
    await tester.tap(find.text('确认'));
    await _pump(tester);
  });

  testWidgets('upgrade-all names the agents that failed', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(760, 720));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final cli = _AgentCli(
      listJson: _agentListJson(),
      versionsJson: _agentVersionsJson(),
      upgradeAllJson: jsonEncode({
        'results': [
          {
            'id': 'codex',
            'cli': {'version': '0.9.1'},
          },
          {'id': 'claude', 'error': 'npm EACCES'},
        ],
      }),
    );
    final controller = MixinController(cli: cli);
    await tester.pumpWidget(
      agentsApp(controller: controller, checkStartupAtLogin: false),
    );
    await _pump(tester);

    await tester.tap(find.text('全部升级'));
    await _pump(tester);
    expect(find.textContaining('已升级：Codex'), findsOneWidget);
    expect(find.textContaining('升级失败 1 个'), findsOneWidget);
    expect(find.textContaining('Claude Code：npm EACCES'), findsOneWidget);
    await tester.tap(find.text('确认'));
    await _pump(tester);
    expect(_called(cli.calls, 'agent upgrade --all --json'), isTrue);
  });

  test('upgrade-all summary reports a failed command verbatim', () {
    expect(
      upgradeAllSummary(
        const CliResult(1, '', 'npm 不可用'),
        (id) => id,
      ),
      '全部升级失败：npm 不可用',
    );
    expect(
      upgradeAllSummary(
        const CliResult(0, '{"results":[]}', ''),
        (id) => id,
      ),
      '全部升级完成',
    );
    // A refusal arrives inside `cli.error` while the command exits 0.
    expect(
      upgradeAllSummary(
        CliResult(
          0,
          jsonEncode({
            'results': [
              {
                'id': 'pi',
                'cli': {'error': "pi's CLI was not found on PATH"},
              },
            ],
          }),
          '',
        ),
        (id) => id,
      ),
      contains('升级失败 1 个'),
    );
  });
}
