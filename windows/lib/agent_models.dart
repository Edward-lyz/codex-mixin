/// Parsed rows of the `agent` CLI JSON contract.
///
/// The CLI owns program detection, integration state and version resolution;
/// the UI renders these fields and never probes the system itself.
enum AgentIntegrationState { managed, unmanaged, error, unsupported, drift }

class AgentCliInfo {
  final String? version;
  final String? latest;
  final String? via;
  final String? path;
  final String? command;
  final String? appVersion;
  final bool update;
  final String? error;

  const AgentCliInfo({
    this.version,
    this.latest,
    this.via,
    this.path,
    this.command,
    this.appVersion,
    this.update = false,
    this.error,
  });

  factory AgentCliInfo.fromJson(Map<String, dynamic> json) => AgentCliInfo(
    version: _stringOrNull(json['version']),
    latest: _stringOrNull(json['latest']),
    via: _stringOrNull(json['via']),
    path: _stringOrNull(json['path']),
    command: _stringOrNull(json['command']),
    appVersion: _stringOrNull(json['app_version']),
    update: json['update'] == true,
    error: _stringOrNull(json['error']),
  );

  /// CLI version, or the desktop app version when the agent ships no CLI
  /// (Claude Desktop reports `app_version`).
  String? get displayVersion => version ?? appVersion;

  bool get upgradable => update && latest != null;
}

class AgentIntegration {
  final AgentIntegrationState state;
  final String? mode;
  final String detail;
  final String? error;

  const AgentIntegration({
    required this.state,
    this.mode,
    this.detail = '',
    this.error,
  });

  factory AgentIntegration.fromJson(Map<String, dynamic>? json) {
    final row = json ?? const <String, dynamic>{};
    return AgentIntegration(
      state: switch ('${row['state'] ?? ''}') {
        'managed' => AgentIntegrationState.managed,
        'error' => AgentIntegrationState.error,
        'unsupported' => AgentIntegrationState.unsupported,
        'drift' => AgentIntegrationState.drift,
        _ => AgentIntegrationState.unmanaged,
      },
      mode: _stringOrNull(row['mode']),
      detail: _stringOrNull(row['detail']) ?? '',
      error: _stringOrNull(row['error']),
    );
  }

  bool get managed =>
      state == AgentIntegrationState.managed ||
      state == AgentIntegrationState.drift;

  bool get canConnect => state != AgentIntegrationState.unsupported;

  String get stateLabel => switch (state) {
    AgentIntegrationState.managed => '已接入',
    AgentIntegrationState.error => '检测错误',
    AgentIntegrationState.unmanaged => '未接入',
    AgentIntegrationState.unsupported => '暂不支持接入',
    AgentIntegrationState.drift => '配置已改变',
  };

  /// Labels for the `--mode` values the CLI accepts; unknown modes render
  /// verbatim so a newer CLI never gets mislabeled.
  String? get modeLabel => switch (mode) {
    null => null,
    'official-account' => '官方账号模式',
    'custom-only' => '仅自定义模型模式',
    final value => value,
  };
}

class AgentInfo {
  final String id;
  final String name;
  final bool detected;
  final AgentIntegration integration;
  final AgentCliInfo? cli;

  const AgentInfo({
    required this.id,
    required this.name,
    required this.detected,
    required this.integration,
    this.cli,
  });

  factory AgentInfo.fromJson(Map<String, dynamic> json) {
    final cli = json['cli'];
    final integration = json['integration'];
    return AgentInfo(
      id: '${json['id'] ?? ''}',
      name: '${json['name'] ?? json['id'] ?? ''}',
      detected: json['detected'] == true,
      integration: AgentIntegration.fromJson(
        integration is Map ? Map<String, dynamic>.from(integration) : null,
      ),
      cli: cli is Map ? AgentCliInfo.fromJson(Map<String, dynamic>.from(cli)) : null,
    );
  }

  AgentInfo withCli(AgentCliInfo? cli) => AgentInfo(
    id: id,
    name: name,
    detected: detected,
    integration: integration,
    cli: cli,
  );

  bool get managed => integration.managed;
  bool get hasUpdate => cli?.upgradable == true;
}

/// Parse the `agents` array shared by `agent list --json` and
/// `agent versions --json`.
List<AgentInfo> parseAgentRows(Object? rows) => (rows is List ? rows : const [])
    .whereType<Map>()
    .map((row) => AgentInfo.fromJson(Map<String, dynamic>.from(row)))
    .toList();

String? _stringOrNull(Object? value) {
  if (value is! String) return null;
  final text = value.trim();
  return text.isEmpty ? null : text;
}
