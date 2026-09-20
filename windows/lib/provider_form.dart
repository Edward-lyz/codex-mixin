import 'dart:io';

import 'package:flutter/material.dart';

import 'models.dart';
import 'theme.dart';

class AddProviderValues {
  final String preset;
  final String name;
  final String customId;
  final String baseUrl;
  final String websiteUrl;
  final String apiKey;
  final String quotaUsername;
  final String workspaceId;
  final String authCookie;
  final String awsRegion;
  final String awsAccessKeyId;
  final String awsSecretAccessKey;
  final String awsSessionToken;
  final bool baiduAuthBridge;

  const AddProviderValues({
    required this.preset,
    required this.name,
    required this.customId,
    required this.baseUrl,
    required this.websiteUrl,
    required this.apiKey,
    required this.quotaUsername,
    required this.workspaceId,
    required this.authCookie,
    required this.awsRegion,
    required this.awsAccessKeyId,
    required this.awsSecretAccessKey,
    required this.awsSessionToken,
    required this.baiduAuthBridge,
  });
}

class AddProviderDialog extends StatefulWidget {
  const AddProviderDialog({super.key});

  @override
  State<AddProviderDialog> createState() => _AddProviderDialogState();
}

class _AddProviderDialogState extends State<AddProviderDialog> {
  final name = TextEditingController(text: '我的供应商');
  final customId = TextEditingController();
  final base = TextEditingController();
  final website = TextEditingController();
  final key = TextEditingController();
  final quotaUsername = TextEditingController();
  final workspaceId = TextEditingController();
  final authCookie = TextEditingController();
  final awsRegion = TextEditingController(text: 'us-east-1');
  final awsAccessKeyId = TextEditingController();
  final awsSecretAccessKey = TextEditingController();
  final awsSessionToken = TextEditingController();
  String preset = 'baidu-oneapi';
  // DUCX auth bridge is off by default when adding OneAPI (matches macOS,
  // which defaults to .disabled); the user opts in explicitly.
  bool baiduAuthBridge = false;

  @override
  void dispose() {
    name.dispose();
    customId.dispose();
    base.dispose();
    website.dispose();
    key.dispose();
    quotaUsername.dispose();
    workspaceId.dispose();
    authCookie.dispose();
    awsRegion.dispose();
    awsAccessKeyId.dispose();
    awsSecretAccessKey.dispose();
    awsSessionToken.dispose();
    super.dispose();
  }

  Future<void> _openWebsite() async {
    final url = providerCredentialUrl(preset);
    if (!url.startsWith('http')) return;
    await Process.start('explorer.exe', [url]);
  }

  @override
  Widget build(BuildContext context) {
    final fields = <Widget>[
      mixinDropdown<String>(
        selected: preset,
        decoration: const InputDecoration(
          labelText: '供应商',
          prefixIcon: Icon(Icons.cloud_outlined),
        ),
        items: const [
          DropdownMenuItem(value: 'baidu-oneapi', child: Text('Baidu OneAPI')),
          DropdownMenuItem(value: 'openrouter', child: Text('OpenRouter')),
          DropdownMenuItem(value: 'deepseek', child: Text('DeepSeek')),
          DropdownMenuItem(value: 'opencode-go', child: Text('OpenCode Go')),
          DropdownMenuItem(value: 'aws-bedrock', child: Text('Amazon Bedrock')),
          DropdownMenuItem(value: 'custom', child: Text('自定义站点')),
        ],
        onChanged: (value) => setState(() => preset = value ?? preset),
      ),
      const SizedBox(height: 4),
      if (providerCredentialUrl(preset).isNotEmpty)
        Align(
          alignment: Alignment.centerLeft,
          child: TextButton.icon(
            onPressed: _openWebsite,
            icon: const Icon(Icons.vpn_key_outlined, size: 18),
            label: const Text('打开密钥页面'),
          ),
        ),
      const SizedBox(height: 12),
    ];
    if (preset == 'custom') {
      fields.addAll([
        TextField(
          controller: name,
          decoration: const InputDecoration(
            labelText: '站点名称',
            prefixIcon: Icon(Icons.title),
          ),
        ),
        const SizedBox(height: 12),
        TextField(
          controller: customId,
          decoration: const InputDecoration(
            labelText: '供应商 ID（小写英文/数字/-/_）',
            prefixIcon: Icon(Icons.tag),
            helperText: '用于命令行标识，例如 my-provider；不能为空或含中文。',
          ),
        ),
        const SizedBox(height: 12),
        TextField(
          controller: base,
          decoration: const InputDecoration(
            labelText: 'API 地址',
            prefixIcon: Icon(Icons.link),
          ),
        ),
        const SizedBox(height: 12),
        TextField(
          controller: website,
          decoration: const InputDecoration(
            labelText: '官网地址',
            prefixIcon: Icon(Icons.public),
          ),
        ),
        const SizedBox(height: 12),
      ]);
    }
    if (preset == 'aws-bedrock') {
      fields.addAll([
        TextField(
          controller: awsRegion,
          decoration: const InputDecoration(
            labelText: 'AWS Region',
            prefixIcon: Icon(Icons.public),
            helperText: 'Endpoint 会按 Region 自动设置为 Bedrock Mantle。',
          ),
        ),
        const SizedBox(height: 12),
        TextField(
          controller: awsAccessKeyId,
          obscureText: true,
          decoration: const InputDecoration(
            labelText: 'Access Key ID',
            prefixIcon: Icon(Icons.key_outlined),
          ),
        ),
        const SizedBox(height: 12),
        TextField(
          controller: awsSecretAccessKey,
          obscureText: true,
          decoration: const InputDecoration(
            labelText: 'Secret Access Key',
            prefixIcon: Icon(Icons.password_outlined),
          ),
        ),
        const SizedBox(height: 12),
        TextField(
          controller: awsSessionToken,
          obscureText: true,
          decoration: const InputDecoration(
            labelText: 'Session Token（可选）',
            prefixIcon: Icon(Icons.vpn_key_outlined),
          ),
        ),
      ]);
    } else {
      fields.add(
        TextField(
          controller: key,
          obscureText: true,
          decoration: const InputDecoration(
            labelText: 'API Key',
            prefixIcon: Icon(Icons.key_outlined),
          ),
        ),
      );
    }
    if (preset == 'baidu-oneapi') {
      fields.addAll([
        const SizedBox(height: 12),
        TextField(
          controller: quotaUsername,
          decoration: const InputDecoration(
            labelText: '额度用户名',
            prefixIcon: Icon(Icons.person_outline),
          ),
        ),
        const SizedBox(height: 12),
        SwitchListTile.adaptive(
          contentPadding: EdgeInsets.zero,
          title: const Text('启用 DUCX 认证桥接'),
          subtitle: const Text('使用百度 DUCX 登录态完成 OneAPI 认证'),
          value: baiduAuthBridge,
          onChanged: (value) => setState(() => baiduAuthBridge = value),
        ),
      ]);
    }
    if (preset == 'opencode-go') {
      fields.addAll([
        const SizedBox(height: 12),
        TextField(
          controller: workspaceId,
          decoration: const InputDecoration(
            labelText: '工作区 ID',
            prefixIcon: Icon(Icons.workspaces_outline),
          ),
        ),
        const SizedBox(height: 12),
        TextField(
          controller: authCookie,
          obscureText: true,
          decoration: const InputDecoration(
            labelText: 'Auth Cookie',
            prefixIcon: Icon(Icons.cookie_outlined),
          ),
        ),
      ]);
    }
    return Dialog(
      insetPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 18),
      backgroundColor: const Color(0xfff1f0f7),
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(20)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 420),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(20, 18, 20, 14),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                '新增供应商',
                style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
              ),
              const SizedBox(height: 12),
              Flexible(
                child: SingleChildScrollView(child: Column(children: fields)),
              ),
              const SizedBox(height: 16),
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  TextButton(
                    onPressed: () => Navigator.pop(context),
                    child: const Text('取消'),
                  ),
                  const SizedBox(width: 8),
                  FilledButton(
                    style: FilledButton.styleFrom(
                      backgroundColor: const Color(0xff4d67a0),
                      foregroundColor: Colors.white,
                      padding: const EdgeInsets.symmetric(
                        horizontal: 22,
                        vertical: 12,
                      ),
                    ),
                    onPressed: () => Navigator.pop(
                      context,
                      AddProviderValues(
                        preset: preset,
                        name: name.text.trim(),
                        customId: customId.text.trim(),
                        baseUrl: base.text.trim(),
                        websiteUrl: website.text.trim(),
                        apiKey: key.text.trim(),
                        quotaUsername: quotaUsername.text.trim(),
                        workspaceId: workspaceId.text.trim(),
                        authCookie: authCookie.text.trim(),
                        awsRegion: awsRegion.text.trim(),
                        awsAccessKeyId: awsAccessKeyId.text.trim(),
                        awsSecretAccessKey: awsSecretAccessKey.text.trim(),
                        awsSessionToken: awsSessionToken.text.trim(),
                        baiduAuthBridge: baiduAuthBridge,
                      ),
                    ),
                    child: const Text('添加'),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}
