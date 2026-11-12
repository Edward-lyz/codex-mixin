using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using SharpVectors.Converters;
using SharpVectors.Renderers.Wpf;

namespace CodexMixin;

internal static class Marks
{
    // UI-thread-only; keys are the fixed set of packaged provider/agent icons.
    private static readonly Dictionary<string, ImageSource> sources = [];
    private static readonly WpfDrawingSettings svgSettings = new() { IncludeRuntime = false, TextAsGeometry = true };

    internal static FrameworkElement Agent(string id)
    {
        if (id.Length == 0 || id.Any(character => !char.IsAsciiLetterOrDigit(character) && character != '-'))
            throw new InvalidOperationException(Localized.Get("CLI 返回了无效 Agent ID。"));
        foreach (var extension in new[] { "png", "svg" })
            if (Source($"agents/{id}.{extension}") is { } source)
                return new Image { Source = source, Width = 32, Height = 32 };
        return new FontIcon { Glyph = "\uE77B", FontSize = 28, Width = 32, Height = 32, TextAlignment = TextAlignment.Center };
    }

    internal static Image Provider(JsonElement provider, double size = 28)
    {
        var name = Json.Text(provider, "icon");
        var agy = name == "agy" || Json.OptionalText(provider, "preset_id") == "agy";
        if (!new[] { "aws", "baidu", "claude", "custom", "deepseek", "kiro", "openai", "opencode", "openrouter" }.Contains(name)) name = "custom";
        return new Image { Source = Source(agy ? "agents/agy.png" : $"providers/{name}.svg"), Width = size, Height = size };
    }

    private static ImageSource? Source(string key)
    {
        if (sources.TryGetValue(key, out var cached)) return cached;
        var path = Path.Combine(AppContext.BaseDirectory, "Assets", key);
        if (!File.Exists(path)) return null;
        ImageSource source;
        if (key.EndsWith(".svg", StringComparison.Ordinal))
        {
            using var reader = new FileSvgReader(svgSettings);
            source = new DrawingImage(reader.Read(path));
        }
        else source = new BitmapImage(new Uri(path));
        source.Freeze();
        return sources[key] = source;
    }
}
