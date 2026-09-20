import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:codex_mixin_ui/controller.dart';
import 'package:codex_mixin_ui/cli.dart';
import 'package:codex_mixin_ui/main.dart';
import 'package:codex_mixin_ui/models.dart';
import 'package:codex_mixin_ui/widgets.dart';

class _RecordingCli extends MixinCli {
  final events = <String>[];

  @override
  Future<CliResult> run(
    List<String> args, {
    Map<String, String>? secrets,
    void Function(String line)? onProgress,
  }) async {
    final label = args.first;
    events.add('start:$label');
    await Future<void>.delayed(Duration.zero);
    events.add('end:$label');
    return const CliResult(0, 'ok', '');
  }
}

void main() {
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
      contains('安装到 Codex'),
    );
  });

  test('formats streamed CLI progress in Chinese', () {
    expect(
      formatCliProgress(
        'MIXIN_PROGRESS Writing Codex config and model catalog',
      ),
      '正在写入 Codex 配置与模型目录',
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
    expect(find.text('保存模型选择'), findsOneWidget);
    expect(find.text('测速'), findsOneWidget);
    expect(find.text('ernie-4.5'), findsOneWidget);
    expect(find.text('ernie-x1'), findsOneWidget);

    final checkbox = tester.widget<Checkbox>(find.byType(Checkbox).at(0));
    expect(checkbox.value, isTrue);
    await tester.tap(find.byType(Checkbox).at(0));
    await tester.pump();
    expect(tester.widget<Checkbox>(find.byType(Checkbox).at(0)).value, isFalse);
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
    expect(find.text('安装与恢复'), findsOneWidget);
    expect(find.text('关于'), findsOneWidget);
    expect(find.text('退出 Codex Mixin'), findsOneWidget);
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

  testWidgets('flyout window renders macOS install and recovery actions', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(280, 520));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(
      flyoutApp(
        controller: MixinController(),
        windowId: 3,
        title: '安装与恢复',
        trayWindowId: 1,
      ),
    );
    await tester.pump();

    expect(find.text('安装到 Codex...'), findsOneWidget);
    expect(find.text('从 Codex 恢复...'), findsOneWidget);
    expect(find.text('安装到 Claude Code...'), findsOneWidget);
    expect(find.text('从 Pi 卸载...'), findsOneWidget);
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
}
