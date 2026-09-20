import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

/// Force Simplified Chinese for all built-in Material/Cupertino widgets (dialog
/// buttons, tooltips, about box, etc.) so no English leaks into the UI.
const mixinLocale = Locale('zh');
const mixinVersion = String.fromEnvironment(
  'CODEX_MIXIN_VERSION',
  defaultValue: 'development build',
);
const mixinSupportedLocales = [Locale('zh'), Locale('en')];
const mixinLocalizationsDelegates = <LocalizationsDelegate<dynamic>>[
  GlobalMaterialLocalizations.delegate,
  GlobalWidgetsLocalizations.delegate,
  GlobalCupertinoLocalizations.delegate,
];

const surface = Color(0xfff1f2f3);
const panel = Color(0xffeceeef);
const line = Color(0xffdfe1e4);
const muted = Color(0xff72767e);
const accent = Color(0xff0869e8);
const green = Color(0xff10a968);
const orange = Color(0xffef9f21);

/// Brand color for each provider icon, so the monochrome SVG glyphs render
/// with recognizable, colored identities instead of all-black.
Color providerIconColor(String icon) {
  switch (icon) {
    case 'baidu':
      return const Color(0xff2932e1);
    case 'openai':
      return const Color(0xff10a37f);
    case 'openrouter':
      return const Color(0xffa855f7);
    case 'deepseek':
      return const Color(0xff4d6bfe);
    case 'opencode':
      return const Color(0xff14b8a6);
    case 'aws':
      return const Color(0xffff9900);
    default:
      return muted;
  }
}

ThemeData mixinTheme() {
  // Windows-first font stack: use the system 微软雅黑 so Chinese renders
  // consistently, with English/Latin fallbacks after it.
  const fontFamily = '微软雅黑';
  const fontFamilyFallback = <String>[
    'Microsoft YaHei UI',
    'Microsoft YaHei',
    'Segoe UI',
  ];
  const textTheme = TextTheme(
    displayLarge: TextStyle(fontSize: 24),
    displayMedium: TextStyle(fontSize: 20),
    displaySmall: TextStyle(fontSize: 17),
    headlineLarge: TextStyle(fontSize: 17, fontWeight: FontWeight.w600),
    headlineMedium: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
    headlineSmall: TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
    titleLarge: TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
    titleMedium: TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
    titleSmall: TextStyle(fontSize: 12, fontWeight: FontWeight.w600),
    bodyLarge: TextStyle(fontSize: 13, height: 1.3),
    bodyMedium: TextStyle(fontSize: 12, height: 1.3),
    bodySmall: TextStyle(fontSize: 11, height: 1.3),
    labelLarge: TextStyle(fontSize: 12, fontWeight: FontWeight.w600),
    labelMedium: TextStyle(fontSize: 11, fontWeight: FontWeight.w600),
    labelSmall: TextStyle(fontSize: 11, fontWeight: FontWeight.w600),
  );
  final colorScheme = ColorScheme.fromSeed(
    seedColor: accent,
  ).copyWith(primary: accent, secondary: const Color(0xff5b8def));
  return ThemeData(
    useMaterial3: true,
    scaffoldBackgroundColor: surface,
    colorScheme: colorScheme,
    fontFamily: fontFamily,
    fontFamilyFallback: fontFamilyFallback,
    textTheme: textTheme.apply(
      fontFamily: fontFamily,
      fontFamilyFallback: fontFamilyFallback,
      bodyColor: const Color(0xff1d2024),
    ),
    visualDensity: VisualDensity.compact,
    splashFactory: NoSplash.splashFactory,
    highlightColor: Colors.transparent,
    hoverColor: Colors.transparent,
    switchTheme: SwitchThemeData(
      thumbColor: WidgetStateProperty.resolveWith((states) {
        if (states.contains(WidgetState.selected)) return Colors.white;
        return const Color(0xfff7f8fa);
      }),
      trackColor: WidgetStateProperty.resolveWith((states) {
        if (states.contains(WidgetState.selected)) {
          return states.contains(WidgetState.hovered)
              ? const Color(0xff0558c7)
              : accent;
        }
        return states.contains(WidgetState.hovered)
            ? const Color(0xffc5c9d0)
            : const Color(0xffd5d8de);
      }),
      overlayColor: WidgetStateProperty.all(Colors.transparent),
      splashRadius: 0,
      trackOutlineColor: WidgetStateProperty.all(Colors.transparent),
    ),
    inputDecorationTheme: InputDecorationTheme(
      isDense: true,
      filled: true,
      fillColor: Colors.white,
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(9),
        borderSide: const BorderSide(color: line),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(9),
        borderSide: const BorderSide(color: line),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(9),
        borderSide: const BorderSide(color: accent, width: 1.5),
      ),
      contentPadding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
    ),
    dialogTheme: DialogThemeData(
      backgroundColor: Colors.white,
      surfaceTintColor: Colors.transparent,
      elevation: 16,
      shadowColor: const Color(0x24000000),
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
      insetPadding: const EdgeInsets.symmetric(horizontal: 40, vertical: 24),
      titleTextStyle: const TextStyle(
        fontFamily: fontFamily,
        fontFamilyFallback: fontFamilyFallback,
        fontSize: 16,
        fontWeight: FontWeight.w700,
        color: Color(0xff1d2024),
      ),
      contentTextStyle: const TextStyle(
        fontFamily: fontFamily,
        fontFamilyFallback: fontFamilyFallback,
        fontSize: 13,
        height: 1.5,
        color: Color(0xff3f4247),
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(
        foregroundColor: accent,
        textStyle: const TextStyle(
          fontSize: 13,
          fontWeight: FontWeight.w600,
        ),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      ),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        textStyle: const TextStyle(
          fontSize: 13,
          fontWeight: FontWeight.w600,
        ),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
        padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 10),
      ),
    ),
  );
}

Widget sectionTitle(String text) => Text(
  text,
  style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
);

Widget sectionBox(Widget child) => Container(
  width: double.infinity,
  padding: const EdgeInsets.all(13),
  decoration: BoxDecoration(
    color: Colors.white,
    borderRadius: BorderRadius.circular(11),
  ),
  child: child,
);

Widget labeledField(
  TextEditingController controller,
  String label,
  IconData icon, {
  String? hint,
  bool obscure = false,
  bool enabled = true,
  ValueChanged<String>? onSubmitted,
}) => TextField(
  controller: controller,
  obscureText: obscure,
  enabled: enabled,
  onSubmitted: onSubmitted,
  decoration: InputDecoration(
    labelText: label,
    hintText: hint,
    prefixIcon: Icon(icon),
  ),
);

Widget statusBadge(String text, Color color) => Container(
  padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
  decoration: BoxDecoration(
    color: color.withValues(alpha: .13),
    borderRadius: BorderRadius.circular(13),
  ),
  child: Text(
    text,
    style: TextStyle(color: color, fontSize: 12, fontWeight: FontWeight.w700),
  ),
);

Widget labeledSwitch({
  required String title,
  required bool value,
  required ValueChanged<bool>? onChanged,
}) => Row(
  children: [
    Expanded(child: Text(title)),
    Transform.scale(
      scale: 0.78,
      alignment: Alignment.centerRight,
      child: Switch(
        materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
        value: value,
        onChanged: onChanged,
      ),
    ),
  ],
);

Widget mixinDropdown<T>({
  required T? selected,
  required List<DropdownMenuItem<T>> items,
  required ValueChanged<T?>? onChanged,
  InputDecoration? decoration,
}) {
  return DropdownButtonFormField<T>(
    // ignore: deprecated_member_use
    value: selected,
    items: items,
    onChanged: onChanged,
    decoration: decoration,
  );
}

String formatTokenCount(Object? value) {
  final count = value is num ? value.toDouble() : 0;
  if (count >= 1000000) return '${(count / 1000000).toStringAsFixed(1)}M';
  if (count >= 1000) return '${(count / 1000).toStringAsFixed(1)}k';
  if (count == count.roundToDouble()) return '${count.round()}';
  return count.toStringAsFixed(1);
}
