class ClientIntegration {
  final String id;
  final String displayName;
  final String removeVerb;
  final List<String> installArguments;
  final List<String> removeArguments;

  const ClientIntegration({
    required this.id,
    required this.displayName,
    this.removeVerb = '恢复',
    required this.installArguments,
    required this.removeArguments,
  });

  String get installLabel => '安装到 $displayName';
  String get removeLabel => '从 $displayName$removeVerb';
}

const clientIntegrations = <ClientIntegration>[
  ClientIntegration(
    id: 'codex',
    displayName: 'Codex',
    installArguments: ['connect', 'codex', '--custom-only'],
    removeArguments: ['connect', 'remove', 'codex'],
  ),
  ClientIntegration(
    id: 'claude',
    displayName: 'Claude Code',
    installArguments: ['connect', 'claude'],
    removeArguments: ['connect', 'remove', 'claude'],
  ),
  ClientIntegration(
    id: 'dsh',
    displayName: 'DSH',
    removeVerb: '卸载',
    installArguments: ['connect', 'dsh'],
    removeArguments: ['connect', 'remove', 'dsh'],
  ),
  ClientIntegration(
    id: 'opencode',
    displayName: 'OpenCode',
    removeVerb: '卸载',
    installArguments: ['connect', 'opencode'],
    removeArguments: ['connect', 'remove', 'opencode'],
  ),
  ClientIntegration(
    id: 'pi',
    displayName: 'Pi',
    removeVerb: '卸载',
    installArguments: ['connect', 'pi'],
    removeArguments: ['connect', 'remove', 'pi'],
  ),
];

({ClientIntegration client, bool install}) clientActionAt(int index) {
  if (index < 0 || index >= clientIntegrations.length * 2) {
    throw RangeError.index(index, clientIntegrations, 'index');
  }
  return (client: clientIntegrations[index ~/ 2], install: index.isEven);
}
