import 'dart:convert';
import 'dart:io';

/// Minimal rotating file logger for the Windows UI process.
///
/// Writes to `%LOCALAPPDATA%\CodexMixin\logs\ui-YYYY-MM-DD_HHMMSS.log` (falling
/// back to `%USERPROFILE%`). A new file is started on the first write of a new
/// day and whenever the active file would exceed [_maxBytes]; rotated files are
/// gzip-compressed into a `history/` sub-folder, keeping the newest
/// [_maxHistory] archives.
class UiLog {
  UiLog._();
  static final UiLog instance = UiLog._();

  static const int _maxBytes = 10 * 1024 * 1024;
  static const int _maxHistory = 10;

  Directory? _dir;
  String _path = '';
  RandomAccessFile? _raf;
  int _len = 0;
  int _day = 0;

  void init() {
    final dir = _resolveLogDir();
    try {
      dir.createSync(recursive: true);
      _dir = dir;
      _openActive(DateTime.now());
    } catch (_) {
      _dir = null;
      _path = '';
      _raf = null;
    }
  }

  String get path => _path;

  /// Directory that holds the log files (used by the UI to open the folder).
  String get directoryPath => _dir?.path ?? '';

  void info(String message) => _write('INFO', message);
  void warn(String message) => _write('WARN', message);
  void error(String message, [Object? error, StackTrace? stack]) {
    final suffix = error == null ? '' : ' | $error';
    _write('ERROR', '$message$suffix');
    if (stack != null) {
      _write('ERROR', stack.toString());
    }
  }

  void _write(String level, String message) {
    if (_raf == null || _dir == null) return;
    final now = DateTime.now();
    final line = '${now.toIso8601String()} [$level] $message\n';
    final data = utf8.encode(line);
    final today = _dayNumber(now);
    if (today != _day || (_len > 0 && _len + data.length > _maxBytes)) {
      _rotate(now);
    }
    final raf = _raf;
    if (raf == null) return;
    try {
      raf.writeFromSync(data);
      raf.flushSync();
      _len += data.length;
    } catch (_) {}
  }

  void _openActive(DateTime now) {
    _day = _dayNumber(now);
    final existing = _reuseToday(now);
    final file =
        existing ??
        File('${_dir!.path}${Platform.pathSeparator}${_activeName(now)}');
    _path = file.path;
    _raf = file.openSync(mode: FileMode.append);
    _len = _raf!.lengthSync();
  }

  void _rotate(DateTime now) {
    final closed = _path;
    try {
      _raf?.closeSync();
    } catch (_) {}
    _raf = null;
    _archive(closed);
    // Fresh, uniquely-named file for the new day/size bucket.
    var file = File(
      '${_dir!.path}${Platform.pathSeparator}${_activeName(now)}',
    );
    for (var i = 1; file.existsSync() && i < 1000; i++) {
      final name = _activeName(now).replaceAll('.log', '-$i.log');
      file = File('${_dir!.path}${Platform.pathSeparator}$name');
    }
    _path = file.path;
    _len = 0;
    try {
      _raf = file.openSync(mode: FileMode.append);
    } catch (_) {
      _raf = null;
    }
  }

  /// Gzip the just-closed file into `history/<name>.gz`, remove the plain file,
  /// then prune the archive set to the newest [_maxHistory] entries.
  void _archive(String sourcePath) {
    try {
      final source = File(sourcePath);
      if (!source.existsSync()) return;
      final history = Directory(
        '${_dir!.path}${Platform.pathSeparator}history',
      );
      history.createSync(recursive: true);
      final name = sourcePath.split(Platform.pathSeparator).last;
      final target = File('${history.path}${Platform.pathSeparator}$name.gz');
      final bytes = source.readAsBytesSync();
      target.writeAsBytesSync(gzip.encode(bytes));
      source.deleteSync();
      _pruneHistory(history);
    } catch (_) {}
  }

  void _pruneHistory(Directory history) {
    try {
      final archives =
          history
              .listSync()
              .whereType<File>()
              .where((f) => f.path.endsWith('.log.gz'))
              .toList()
            ..sort((a, b) => a.path.compareTo(b.path));
      final remove = archives.length - _maxHistory;
      for (var i = 0; i < remove; i++) {
        try {
          archives[i].deleteSync();
        } catch (_) {}
      }
    } catch (_) {}
  }

  /// Reuse today's newest file when it still has room, so a restart within the
  /// same day appends instead of spawning a new file.
  File? _reuseToday(DateTime now) {
    try {
      final prefix = _datePrefix(now);
      File? best;
      String? bestName;
      for (final entry in _dir!.listSync().whereType<File>()) {
        final name = entry.path.split(Platform.pathSeparator).last;
        if (!name.startsWith(prefix) || !name.endsWith('.log')) continue;
        if (bestName == null || name.compareTo(bestName) > 0) {
          bestName = name;
          best = entry;
        }
      }
      if (best != null && best.lengthSync() < _maxBytes) return best;
    } catch (_) {}
    return null;
  }

  String _activeName(DateTime t) {
    final d =
        '${t.year.toString().padLeft(4, '0')}-${_two(t.month)}-${_two(t.day)}';
    final time = '${_two(t.hour)}${_two(t.minute)}${_two(t.second)}';
    return 'ui-${d}_$time.log';
  }

  String _datePrefix(DateTime t) =>
      'ui-${t.year.toString().padLeft(4, '0')}-${_two(t.month)}-${_two(t.day)}_';

  int _dayNumber(DateTime t) => t.year * 10000 + t.month * 100 + t.day;

  String _two(int n) => n.toString().padLeft(2, '0');

  Directory _resolveLogDir() {
    final base =
        Platform.environment['LOCALAPPDATA'] ??
        Platform.environment['USERPROFILE'] ??
        '.';
    return Directory(
      '$base${Platform.pathSeparator}CodexMixin${Platform.pathSeparator}logs',
    );
  }
}
