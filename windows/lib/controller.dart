import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

import 'cli.dart';
import 'log.dart';
import 'models.dart';

class MixinController {
  MixinController({MixinCli? cli}) : cli = cli ?? MixinCli();

  final MixinCli cli;
  GatewaySnapshot snapshot = GatewaySnapshot.empty;
  int? usageDays = 1;
  bool launchAtLogin = false;
  bool busy = false;
  String status = GatewaySnapshot.empty.status;
  String? gatewayLogPath;
  String? stateDirectory;
  Future<void>? _refreshInFlight;
  Future<void> _actionTail = Future<void>.value();

  Future<void> refresh({bool force = false}) {
    final pending = _refreshInFlight;
    // Coalesce overlapping periodic refreshes onto the in-flight one so a slow
    // earlier response cannot land after and overwrite newer data. Forced
    // refreshes (after a mutating action or range change) queue behind the
    // current one and then run fresh so the latest state always wins.
    if (pending != null && !force) return pending;
    final run = (pending ?? Future<void>.value()).then((_) => _runRefresh());
    _refreshInFlight = run;
    run.whenComplete(() {
      if (identical(_refreshInFlight, run)) _refreshInFlight = null;
    });
    return run;
  }

  Future<void> _runRefresh() async {
    final results = await Future.wait([
      cli.run(['provider', 'list', '--json']),
      cli.run(['service', 'status', '--json']),
      cli.run(['interface', '--json']),
      cli.run(['quota', '--json']),
      cli.run([
        'usage',
        '--json',
        if (usageDays != null) ...['--days', '$usageDays'],
      ]),
    ]);
    final providerResult = results[0];
    final statusResult = results[1];
    final interfaceResult = results[2];
    final quotaResult = results[3];
    final usageResult = results[4];
    final providersJson = decodeCliObject(providerResult.stdout);
    final statusJson = statusResult.ok
        ? decodeCliObject(statusResult.stdout)
        : null;
    final interfaceJson = interfaceResult.ok
        ? decodeCliObject(interfaceResult.stdout)
        : null;
    final paths = interfaceJson?['paths'];
    if (paths is Map) {
      final statePath = paths['state'];
      final gatewayPath = paths['gateway_log'];
      if (statePath is String && statePath.isNotEmpty) {
        stateDirectory = statePath;
      }
      if (gatewayPath is String && gatewayPath.isNotEmpty) {
        gatewayLogPath = gatewayPath;
      }
    }
    final providerRows = providersJson?['providers'];
    final providers = (providerRows is List ? providerRows : const [])
        .whereType<Map>()
        .map(
          (value) => ProviderModel.fromJson(Map<String, dynamic>.from(value)),
        )
        .toList();
    final gatewayRunning =
        statusResult.ok && statusJson?['gateway'] == 'running';
    final endpoint = statusJson?['endpoint'];
    final logPath = statusJson?['log'];
    if (logPath is String && logPath.isNotEmpty) gatewayLogPath = logPath;
    final quota = decodeCliJson(quotaResult.stdout);
    final usage = decodeCliJson(usageResult.stdout);
    final usageError = !usageResult.ok
        ? usageResult.output
        : usage is List
        ? null
        : 'usage 返回了无法解析的数据';
    final ech = statusJson?['official_ech'];
    final echReason = ech is Map ? ech['fallback_reason'] : null;
    final String statusText;
    if (!providerResult.ok) {
      statusText = '读取供应商失败：${providerResult.output}';
    } else if (providersJson == null || providerRows is! List) {
      statusText = '供应商接口返回了无效 JSON';
    } else if (!statusResult.ok) {
      statusText = '网关状态查询失败：${statusResult.output}';
    } else if (statusJson == null || statusJson['gateway'] is! String) {
      statusText = '网关状态接口返回了无效 JSON';
    } else if (!interfaceResult.ok) {
      statusText = '读取应用信息失败：${interfaceResult.output}';
    } else if (interfaceJson == null || interfaceJson['protocol_version'] != 1) {
      statusText = '应用信息返回了无效 JSON';
    } else {
      statusText = echReason is String
          ? 'ECH 已自动关闭并回退到直连：$echReason'
          : (providers.isEmpty ? '还没有供应商' : '配置已同步');
    }
    snapshot = GatewaySnapshot(
      providers: providers,
      gatewayRunning: gatewayRunning,
      serviceTitle: gatewayRunning
          ? (echReason is String ? '本地网关运行中 · ECH 已回退直连' : '本地网关运行中')
          : (statusResult.ok ? '本地网关已停止' : '无法连接本地网关'),
      officialEchEnabled: ech is Map && ech['enabled'] == true,
      serviceEndpoint: endpoint is String && endpoint.isNotEmpty
          ? endpoint
          // Do not keep showing a previous endpoint when the status query
          // fails; fall back to the neutral default so the UI does not imply a
          // live gateway that we can no longer reach.
          : (statusResult.ok
                ? snapshot.serviceEndpoint
                : GatewaySnapshot.empty.serviceEndpoint),
      quotaRows: (quota is List ? quota : const [])
          .whereType<Map>()
          .map((row) => Map<String, dynamic>.from(row))
          .toList(),
      usageRows: (usage is List ? usage : const [])
          .whereType<Map>()
          .map((row) => Map<String, dynamic>.from(row))
          .toList(),
      usageError: usageError,
      status: statusText,
    );
    status = snapshot.status;
  }

  Future<CliResult> runAction(
    String label,
    List<String> args, {
    Map<String, String>? secrets,
    Duration? timeout,
    void Function(String line)? onProgress,
  }) {
    // Serialize actions through a shared tail so concurrent provider writes
    // cannot interleave, without returning a false "busy" failure.
    final run = _actionTail.then(
      (_) => _runAction(
        label,
        args,
        secrets: secrets,
        timeout: timeout,
        onProgress: onProgress,
      ),
    );
    _actionTail = run.then<void>((_) {}, onError: (_) {});
    return run;
  }

  Future<CliResult> _runAction(
    String label,
    List<String> args, {
    Map<String, String>? secrets,
    Duration? timeout,
    void Function(String line)? onProgress,
  }) async {
    busy = true;
    status = '$label…';
    // Persist UI-triggered actions to the log. Args only ever carry `@env:NAME`
    // placeholders for secrets (real values go through the process env), so they
    // are safe to record; on failure we also capture the full CLI output so the
    // cause is diagnosable from the log instead of only the dialog.
    UiLog.instance.info('action start: $label args=$args');
    try {
      final call = cli.run(args, secrets: secrets, onProgress: onProgress);
      final result = timeout == null ? await call : await call.timeout(timeout);
      status = result.ok ? '$label完成' : '$label失败：${result.output}';
      if (result.ok) {
        UiLog.instance.info('action ok: $label (exit ${result.code})');
      } else {
        UiLog.instance.warn(
          'action failed: $label (exit ${result.code})\n'
          'stdout: ${result.stdout.trim()}\n'
          'stderr: ${result.stderr.trim()}',
        );
      }
      return result;
    } on TimeoutException {
      status = '$label超时，请稍后重试';
      UiLog.instance.warn('action timeout: $label');
      return const CliResult(1, '', '操作超时');
    } catch (error, stack) {
      status = '$label失败：$error';
      UiLog.instance.error('action error: $label', error, stack);
      return CliResult(1, '', '$error');
    } finally {
      busy = false;
    }
  }

  Future<bool> _gatewayRunning() async {
    final status = await cli.run(['service', 'status', '--json']);
    final json = decodeCliObject(status.stdout);
    if (!status.ok) throw FormatException(status.output);
    if (json == null || json['gateway'] is! String) {
      throw const FormatException('网关状态接口返回了无效 JSON');
    }
    return json['gateway'] == 'running';
  }

  Future<void> toggleGateway() async {
    if (snapshot.gatewayRunning) {
      final result = await runAction('停止网关', [
        'service',
        'stop',
        '--managed',
        '--json',
      ]);
      final error = decodeCliObject(result.stderr)?['error'];
      if (error is Map && error['code'] == 'codex_requires_gateway') {
        status = '停止网关失败：Codex 仍通过 Mixin 网关连接 GPT。'
            '请先在“安装与恢复”中从 Codex 恢复，再停止网关。';
      }
    } else {
      await runAction('启动网关', ['service', 'start', '--managed', '--json']);
    }
    await refresh(force: true);
  }

  Future<void> refreshLaunchAtLogin() async {
    if (!Platform.isWindows) return;
    final result = await Process.run('reg.exe', [
      'query',
      r'HKCU\Software\Microsoft\Windows\CurrentVersion\Run',
      '/v',
      'CodexMixin',
    ]);
    launchAtLogin = result.exitCode == 0;
  }

  Future<void> toggleLaunchAtLogin() async {
    final key = r'HKCU\Software\Microsoft\Windows\CurrentVersion\Run';
    final args = launchAtLogin
        ? ['delete', key, '/v', 'CodexMixin', '/f']
        : [
            'add',
            key,
            '/v',
            'CodexMixin',
            '/t',
            'REG_SZ',
            '/d',
            '"${Platform.resolvedExecutable}"',
            '/f',
          ];
    final result = await Process.run('reg.exe', args);
    if (result.exitCode == 0) {
      launchAtLogin = !launchAtLogin;
      UiLog.instance.info('launch at login -> $launchAtLogin');
      if (launchAtLogin && !snapshot.gatewayRunning) await toggleGateway();
    } else {
      status = '设置登录启动失败：${result.stderr}';
      UiLog.instance.warn(
        'toggle launch at login failed (exit ${result.exitCode}): '
        '${'${result.stderr}'.trim()}',
      );
    }
  }

  Future<CliResult> doctor() =>
      runAction('健康检测和修复', ['doctor', '--fix', '--quick']);

  Future<CliResult> install(String target, List<String> args) =>
      runAction('安装到 $target', args);

  Future<CliResult> restore(String target, List<String> args) =>
      runAction('从 $target 恢复', args);

  Future<void> copyEndpoint() async {
    await Clipboard.setData(ClipboardData(text: snapshot.serviceEndpoint));
    status = '本地接口地址已复制';
    UiLog.instance.info('copied endpoint: ${snapshot.serviceEndpoint}');
  }

  Future<void> openLogs() async {
    // Log files are now datetime-stamped and rotated, so open the folder that
    // holds them (gateway + ui logs and the history/ archives) rather than a
    // single fixed file. Prefer the gateway log dir reported by the CLI; fall
    // back to the UI log dir, then the state directory.
    final gatewayPath = gatewayLogPath;
    String? dirPath;
    if (gatewayPath != null && gatewayPath.isNotEmpty) {
      dirPath = File(gatewayPath).parent.path;
    } else if (UiLog.instance.directoryPath.isNotEmpty) {
      dirPath = UiLog.instance.directoryPath;
    } else {
      dirPath = UiLog.instance.directoryPath;
    }
    final dir = Directory(dirPath);
    if (dir.existsSync()) {
      UiLog.instance.info('open logs folder: ${dir.path}');
      await Process.start('explorer.exe', [dir.path]);
    } else {
      status = '日志目录还不存在';
      UiLog.instance.warn('open logs folder skipped; missing: $dirPath');
    }
  }

  Future<void> openConfigFolder() async {
    final path = stateDirectory;
    if (path == null || path.isEmpty) {
      status = '无法读取配置目录路径';
      return;
    }
    final dir = Directory(path);
    if (!await dir.exists()) {
      status = '配置目录还不存在';
      return;
    }
    UiLog.instance.info('open config folder: ${dir.path}');
    await Process.start('explorer.exe', [dir.path]);
  }

  Future<void> openReleasePage() async {
    const url = 'https://github.com/Edward-lyz/codex-mixin/releases/latest';
    try {
      await Process.start('explorer.exe', [url]);
      status = '已打开最新版本下载页面';
      UiLog.instance.info('open release page: $url');
    } catch (error, stack) {
      status = '打开更新页面失败：$error';
      UiLog.instance.error('open release page failed', error, stack);
    }
  }

  Future<CliResult> exportConfig(String path) async {
    final result = await runAction('导出配置备份', ['config', 'export', path]);
    if (result.ok && File(path).existsSync()) {
      await Process.start('explorer.exe', ['/select,', path]);
    }
    return result;
  }

  Future<CliResult> importConfig(String path) async {
    final imported = await runAction('导入配置备份', [
      'config',
      'import',
      path,
    ]);
    if (!imported.ok) return imported;
    final restarted = await runAction('应用导入配置', [
      'service',
      'restart',
      '--managed',
      '--json',
    ]);
    await refresh(force: true);
    return restarted.ok ? imported : restarted;
  }

  Future<CliResult> reportReplay() async {
    // The gateway mints and persists the DUCX report token during its startup
    // warmup, so try the replay directly first. Do NOT clear the token we may
    // already have (the previous --prepare-warmup flow threw away a valid token
    // and then failed to re-mint it under the restart race).
    var result = await runAction('手动触发上报', ['report-replay', '--all-sessions']);
    if (result.ok) return result;
    // Token likely missing: restart the gateway so a fresh startup warmup mints
    // it, give the warmup a moment to persist, then retry once.
    await runAction('重启网关', [
      'service',
      'restart',
      '--managed',
      '--json',
    ]);
    await Future<void>.delayed(const Duration(seconds: 4));
    result = await runAction('手动触发上报', ['report-replay', '--all-sessions']);
    return result;
  }

  Future<void> restartGateway() async {
    final result = await runAction('重启网关', [
      'service',
      'restart',
      '--managed',
      '--json',
    ]);
    await refresh(force: true);
    if (!result.ok) {
      status = '重启网关失败：${result.output}';
    }
  }

  /// Start the gateway when it is not already running. Used on launch-at-login
  /// startup so enabling "start service at login" actually brings the gateway
  /// up on the next sign-in, not just when the toggle is flipped.
  Future<void> startGatewayIfStopped() async {
    if (await _gatewayRunning()) return;
    await runAction('启动网关', ['service', 'start', '--managed', '--json']);
    await refresh(force: true);
  }
}
