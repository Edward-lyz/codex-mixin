import 'dart:io';

const _dialogPrelude = r'''
Add-Type -AssemblyName System.Windows.Forms
$dialog = New-Object System.Windows.Forms.''';

String buildBackupDialogScript(String dialogClass, String dialogBody) =>
    '$_dialogPrelude$dialogClass\n$dialogBody';

Future<String?> chooseBackupExportPath() => _chooseBackup(
  'SaveFileDialog',
  r'''
$dialog.Title = '导出配置备份'
$dialog.Filter = 'Codex Mixin Base64 backup (*.b64)|*.b64|All files (*.*)|*.*'
$dialog.FileName = 'codex-mixin-config.b64'
if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
  [Console]::Out.Write($dialog.FileName)
}
''',
);

Future<String?> chooseBackupImportPath() => _chooseBackup(
  'OpenFileDialog',
  r'''
$dialog.Title = '导入配置备份'
$dialog.Filter = 'Codex Mixin Base64 backup (*.b64)|*.b64|All files (*.*)|*.*'
$dialog.Multiselect = $false
if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
  [Console]::Out.Write($dialog.FileName)
}
''',
);

Future<String?> _chooseBackup(String dialogClass, String dialogBody) async {
  final result = await Process.run('powershell.exe', [
    '-NoProfile',
    '-STA',
    '-NonInteractive',
    '-Command',
    buildBackupDialogScript(dialogClass, dialogBody),
  ]);
  if (result.exitCode != 0) {
    throw StateError('Windows file picker failed: ${result.stderr}');
  }
  final path = '${result.stdout}'.trim();
  return path.isEmpty ? null : path;
}
