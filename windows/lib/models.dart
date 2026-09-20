import 'theme.dart';
import 'package:flutter/material.dart';

class ProviderModel {
  final String id;
  final String displayName;
  final String kind;
  final String icon;
  final bool enabled;
  final String preset;
  final String protocol;
  final String baseUrl;
  final String? websiteUrl;
  final String? apiPath;
  final String? imageGenerationPath;
  final bool apiKeyConfigured;
  final bool awsSigv4Configured;
  final String? awsRegion;
  final bool awsSessionTokenConfigured;
  final List<String> selectedModels;
  final List<Map<String, dynamic>> cachedModels;
  final String readiness;
  final List<String> readinessIssues;
  final int routableModelCount;
  final int newModelsCount;
  final int unavailableCount;
  final String? lastRefreshError;
  final bool auxiliaryModelUpstream;
  final String? quotaUsername;
  final String? quotaWorkspaceId;
  final bool quotaAuthCookieConfigured;
  final String? quotaCurrency;
  final String? baiduAuthBridge;
  final bool? baiduCodeReport;

  const ProviderModel({
    required this.id,
    required this.displayName,
    required this.kind,
    required this.icon,
    required this.enabled,
    required this.preset,
    required this.protocol,
    required this.baseUrl,
    required this.websiteUrl,
    required this.apiPath,
    required this.imageGenerationPath,
    required this.apiKeyConfigured,
    required this.awsSigv4Configured,
    required this.awsRegion,
    required this.awsSessionTokenConfigured,
    required this.selectedModels,
    required this.cachedModels,
    required this.readiness,
    required this.readinessIssues,
    required this.routableModelCount,
    required this.newModelsCount,
    required this.unavailableCount,
    required this.lastRefreshError,
    required this.auxiliaryModelUpstream,
    required this.quotaUsername,
    required this.quotaWorkspaceId,
    required this.quotaAuthCookieConfigured,
    required this.quotaCurrency,
    required this.baiduAuthBridge,
    required this.baiduCodeReport,
  });

  factory ProviderModel.fromJson(Map<String, dynamic> json) {
    List<String> strings(String key) =>
        (json[key] as List? ?? const []).map((value) => '$value').toList();
    final cached = (json['cached_models'] as List? ?? const [])
        .whereType<Map>()
        .map((value) => Map<String, dynamic>.from(value))
        .toList();
    return ProviderModel(
      id: '${json['id'] ?? ''}',
      displayName: '${json['display_name'] ?? json['id'] ?? ''}',
      kind: '${json['kind'] ?? 'configured'}',
      icon: '${json['icon'] ?? 'custom'}',
      enabled: json['enabled'] != false,
      preset: '${json['preset_id'] ?? 'custom'}',
      protocol: '${json['protocol'] ?? 'open_ai_responses'}',
      baseUrl: '${json['base_url'] ?? ''}',
      websiteUrl: json['website_url'] as String?,
      apiPath: json['api_path'] as String?,
      imageGenerationPath: json['image_generation_path'] as String?,
      apiKeyConfigured: json['api_key_configured'] == true,
      awsSigv4Configured: json['aws_sigv4_configured'] == true,
      awsRegion: json['aws_region'] as String?,
      awsSessionTokenConfigured: json['aws_session_token_configured'] == true,
      selectedModels: strings('selected_models'),
      cachedModels: cached,
      readiness: '${json['readiness'] ?? 'unknown'}',
      readinessIssues: strings('readiness_issues'),
      routableModelCount: (json['routable_model_count'] as num?)?.toInt() ?? 0,
      newModelsCount: (json['new_models'] as List?)?.length ?? 0,
      unavailableCount:
          (json['unavailable_selected_models'] as List?)?.length ?? 0,
      lastRefreshError: json['last_model_refresh_error'] as String?,
      auxiliaryModelUpstream: json['auxiliary_model_upstream'] == true,
      quotaUsername: json['quota_username'] as String?,
      quotaWorkspaceId: json['quota_workspace_id'] as String?,
      quotaAuthCookieConfigured: json['quota_auth_cookie_configured'] == true,
      quotaCurrency: json['quota_currency'] as String?,
      baiduAuthBridge: json['baidu_auth_bridge'] as String?,
      baiduCodeReport: json['baidu_code_report'] as bool?,
    );
  }

  bool get official => kind == 'official';
  bool get isBaiduOneApi => preset == 'baidu-oneapi';
  bool get isOpenCodeGo => preset == 'opencode-go';
  bool get isAwsBedrock => preset == 'aws-bedrock';
  bool get isCustom => preset == 'custom';
  String get stateLabel =>
      official ? '官方' : (enabled ? readinessLabel(readiness) : '已停用');
  String get modelSummary => official
      ? '官方供应商 · 只读'
      : '$stateLabel · ${selectedModels.length}/${cachedModels.length} 个模型';
  Color get statusColor =>
      enabled ? (readiness == 'degraded' ? orange : green) : Colors.grey;
}

String readinessLabel(String readiness) {
  switch (readiness) {
    case 'healthy':
      return '正常';
    case 'degraded':
      return '降级';
    case 'disabled':
      return '停用';
    default:
      return readiness;
  }
}

String knownWebsite(ProviderModel provider) =>
    knownWebsiteForPreset(provider.preset, websiteUrl: provider.websiteUrl);

/// The "打开密钥页面" target per preset, matching macOS `providerCredentialURL`
/// exactly. Returns an empty string for custom/unknown presets (no button).
String providerCredentialUrl(String preset) {
  return switch (preset) {
    'baidu-oneapi' => 'https://oneapi-comate.baidu-int.com/token',
    'openrouter' => 'https://openrouter.ai/settings/keys',
    'deepseek' => 'https://platform.deepseek.com/api_keys',
    'opencode-go' => 'https://opencode.ai/go',
    'aws-bedrock' => 'https://aws.amazon.com/bedrock/',
    _ => '',
  };
}

/// Resolve a provider website from its preset (and an optional explicit URL).
/// Shared by the provider detail view and the add-provider dialog, which does
/// not yet have a [ProviderModel] to hand to [knownWebsite].
String knownWebsiteForPreset(String preset, {String? websiteUrl}) {
  if (websiteUrl != null && websiteUrl.trim().isNotEmpty) {
    return websiteUrl.trim();
  }
  return switch (preset) {
    'openrouter' => 'https://openrouter.ai',
    'deepseek' => 'https://www.deepseek.com',
    'baidu-oneapi' => 'https://comate.baidu.com',
    'opencode-go' => 'https://opencode.ai',
    'aws-bedrock' => 'https://aws.amazon.com/bedrock',
    _ => '内置供应商',
  };
}

class GatewaySnapshot {
  final List<ProviderModel> providers;
  final bool gatewayRunning;
  final String serviceTitle;
  final String serviceEndpoint;
  final List<Map<String, dynamic>> quotaRows;
  final List<Map<String, dynamic>> usageRows;
  final String? usageError;
  final String status;

  const GatewaySnapshot({
    required this.providers,
    required this.gatewayRunning,
    required this.serviceTitle,
    required this.serviceEndpoint,
    required this.quotaRows,
    required this.usageRows,
    this.usageError,
    required this.status,
  });

  static const empty = GatewaySnapshot(
    providers: [],
    gatewayRunning: false,
    serviceTitle: '本地网关已停止',
    serviceEndpoint: 'http://127.0.0.1:64088/v1',
    quotaRows: [],
    usageRows: [],
    usageError: null,
    status: '正在读取供应商…',
  );
}
