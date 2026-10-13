import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_svg/flutter_svg.dart';

/// The same offline brand assets and light/dark rules as the macOS app.
class AgentLogo extends StatelessWidget {
  final String agentID;
  final double size;

  const AgentLogo({super.key, required this.agentID, this.size = 24});

  static final _manifest = rootBundle
      .loadString('assets/agents/manifest.json')
      .then((text) => jsonDecode(text) as Map<String, dynamic>);

  @override
  Widget build(BuildContext context) => SizedBox(
    width: size,
    height: size,
    child: ExcludeSemantics(
      child: FutureBuilder<Map<String, dynamic>>(
        future: _manifest,
        builder: (context, snapshot) {
          if (snapshot.hasError) {
            return Icon(Icons.broken_image_outlined, size: size);
          }
          if (!snapshot.hasData) return const SizedBox.shrink();
          final asset = snapshot.data![agentID] as Map<String, dynamic>?;
          if (asset == null) {
            return Icon(Icons.broken_image_outlined, size: size);
          }
          final path = 'assets/agents/${asset['file']}';
          if (path.endsWith('.png')) {
            return Image.asset(path, width: size, height: size, fit: BoxFit.contain);
          }
          final dark = Theme.of(context).brightness == Brightness.dark;
          return SvgPicture.asset(
            path,
            width: size,
            height: size,
            fit: BoxFit.contain,
            colorFilter: asset['template'] == true
                ? ColorFilter.mode(
                    dark ? const Color(0xffeeeeee) : const Color(0xff303030),
                    BlendMode.srcIn,
                  )
                : null,
          );
        },
      ),
    ),
  );
}
