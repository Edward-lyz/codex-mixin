import 'dart:convert';
import 'dart:io';

import 'log.dart';

class CliResult {
  final int code;
  final String stdout;
  final String stderr;

  const CliResult(this.code, this.stdout, this.stderr);
  String get output => stderr.trim().isNotEmpty ? stderr.trim() : stdout.trim();
  bool get ok => code == 0;
}

dynamic decodeCliJson(String output) {
  final value = output.trim();
  if (value.isEmpty) return null;
  try {
    return jsonDecode(value);
  } catch (_) {}
  for (var index = 0; index < value.length; index++) {
    if (value[index] != '{' && value[index] != '[') continue;
    try {
      return jsonDecode(value.substring(index));
    } catch (_) {}
  }
  return null;
}

Map<String, dynamic>? decodeCliObject(String output) {
  final value = decodeCliJson(output);
  return value is Map<String, dynamic> ? value : null;
}

class MixinCli {
  Directory? root;
  File? executable;
  final Map<String, DateTime> _recentFailures = {};

  MixinCli() {
    root = _resolveRoot();
    executable = _resolveCli();
  }

  Directory _resolveRoot() {
    final candidates = <Directory>[Directory.current];
    var current = Directory(File(Platform.resolvedExecutable).parent.path);
    for (var i = 0; i < 8; i++) {
      candidates.add(current);
      current = current.parent;
    }
    for (final candidate in candidates) {
      if (File(
            '${candidate.path}${Platform.pathSeparator}Cargo.toml',
          ).existsSync() ||
          File(
            '${candidate.path}${Platform.pathSeparator}codex-mixin.exe',
          ).existsSync()) {
        return candidate;
      }
    }
    return Directory.current;
  }

  File? _resolveCli() {
    final candidates = [
      File('${root!.path}${Platform.pathSeparator}codex-mixin.exe'),
      File(
        '${root!.path}${Platform.pathSeparator}target${Platform.pathSeparator}release${Platform.pathSeparator}codex-mixin.exe',
      ),
      File(
        '${root!.path}${Platform.pathSeparator}target${Platform.pathSeparator}debug${Platform.pathSeparator}codex-mixin.exe',
      ),
      File(
        '${Platform.environment['LOCALAPPDATA'] ?? ''}${Platform.pathSeparator}CodexMixin${Platform.pathSeparator}codex-mixin.exe',
      ),
    ];
    return candidates.where((file) => file.existsSync()).firstOrNull;
  }

  Future<CliResult> run(
    List<String> args, {
    Map<String, String>? secrets,
    void Function(String line)? onProgress,
  }) async {
    final cli = executable;
    if (cli == null) {
      return const CliResult(1, '', '找不到 codex-mixin.exe，请使用发布包中的程序。');
    }
    // Secret values are passed through the child process environment and
    // referenced from argv as `@env:NAME`, so credentials never appear on the
    // Windows command line where same-user tooling could read them.
    Map<String, String>? environment;
    if (secrets != null && secrets.isNotEmpty) {
      environment = {...Platform.environment, ...secrets};
    }
    final process = await Process.start(
      cli.path,
      ['--no-tui', ...args],
      workingDirectory: root!.path,
      environment: environment,
    );
    final stdoutBuffer = StringBuffer();
    final stderrBuffer = StringBuffer();
    // Use a lenient decoder: DUCX/console output can contain non-UTF-8 bytes
    // (e.g. GBK), and a multi-byte char can be split across stream chunks. A
    // strict decoder would throw FormatException and fail the whole action.
    const decoder = Utf8Decoder(allowMalformed: true);
    final stdoutDone = process.stdout
        .transform(decoder)
        .forEach(stdoutBuffer.write);
    final stderrDone = process.stderr
        .transform(decoder)
        .transform(const LineSplitter())
        .forEach((line) {
          stderrBuffer.writeln(line);
          if (onProgress != null && line.startsWith('MIXIN_PROGRESS ')) {
            onProgress(line);
          }
        });
    final exitCode = await process.exitCode;
    // `service start` can leave inherited stdio handles open, so the stream
    // futures may never complete; bound the wait so the queue never blocks.
    await Future.wait([
      stdoutDone,
      stderrDone,
    ]).timeout(const Duration(seconds: 2), onTimeout: () => <void>[]);
    final result = CliResult(
      exitCode,
      stdoutBuffer.toString(),
      stderrBuffer.toString(),
    );
    if (!result.ok) _logFailure(args, result);
    return result;
  }

  /// Record any failed CLI invocation — including the background refresh/status
  /// polls that never go through the action logger — so problems are always
  /// diagnosable from the log. Identical failures are collapsed for a minute so
  /// a persistently failing poll (e.g. gateway down) cannot flood the log.
  void _logFailure(List<String> args, CliResult result) {
    final key = '${args.join(' ')}|${result.code}|${result.output}';
    final now = DateTime.now();
    final last = _recentFailures[key];
    if (last != null && now.difference(last) < const Duration(seconds: 60)) {
      return;
    }
    _recentFailures[key] = now;
    _recentFailures.removeWhere(
      (_, t) => now.difference(t) > const Duration(minutes: 5),
    );
    UiLog.instance.warn(
      'cli failed: ${args.join(' ')} (exit ${result.code})\n${result.output}',
    );
  }

  String get stateDirectory =>
      '${Platform.environment['USERPROFILE'] ?? '.'}${Platform.pathSeparator}.codex-mixin';
}
