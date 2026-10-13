import 'dart:convert';

import 'package:codex_mixin_ui/agent_logo.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('all 50 agent logos load in light and dark themes', (tester) async {
    final manifest = jsonDecode(
      await rootBundle.loadString('assets/agents/manifest.json'),
    ) as Map<String, dynamic>;
    expect(manifest, hasLength(50));
    for (final brightness in [Brightness.light, Brightness.dark]) {
      for (final entry in manifest.entries) {
        await tester.pumpWidget(MaterialApp(
          theme: ThemeData(brightness: brightness),
          home: Center(child: AgentLogo(agentID: entry.key)),
        ));
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull, reason: '${entry.key} $brightness');
        expect(find.byIcon(Icons.broken_image_outlined), findsNothing,
            reason: entry.key);
        final bytes = await rootBundle.load('assets/agents/${entry.value['file']}');
        expect(bytes.lengthInBytes, greaterThan(0));
      }
    }
  });
}
