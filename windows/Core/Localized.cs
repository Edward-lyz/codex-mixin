using System.Globalization;
using System.Resources;

namespace CodexMixin;

// Interface text. The Chinese source text is the key; Strings.resx holds the English text.
// CODEX_MIXIN_LANG (zh or en) overrides the system UI language.
internal static class Localized
{
    private static readonly ResourceManager English = new("CodexMixin.Strings", typeof(Localized).Assembly);

    internal static bool UseEnglish { get; } = DetectEnglish();

    private static bool DetectEnglish()
    {
        var forced = Environment.GetEnvironmentVariable("CODEX_MIXIN_LANG");
        if (!string.IsNullOrWhiteSpace(forced)) return !forced.StartsWith("zh", StringComparison.OrdinalIgnoreCase);
        return CultureInfo.CurrentUICulture.TwoLetterISOLanguageName != "zh";
    }

    internal static string Get(string text) =>
        UseEnglish ? English.GetString(text, CultureInfo.InvariantCulture) ?? text : text;

    internal static string Format(string text, params object?[] args) =>
        string.Format(CultureInfo.CurrentCulture, Get(text), args);
}
