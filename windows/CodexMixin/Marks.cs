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
    private static readonly Dictionary<string, bool> monochrome = [];
    private static readonly WpfDrawingSettings svgSettings = new() { IncludeRuntime = false, TextAsGeometry = true };

    internal static FrameworkElement Agent(string id, double size = 32)
    {
        if (id.Length == 0 || id.Any(character => !char.IsAsciiLetterOrDigit(character) && character != '-'))
            throw new InvalidOperationException(Localized.Get("CLI 返回了无效 Agent ID。"));
        foreach (var extension in new[] { "png", "svg" })
            if (Source($"agents/{id}.{extension}") is { } source)
            {
                var key = $"agents/{id}.{extension}";
                return IsMonochromeAgent(key, id) ? ThemedMark(source, size) : new Image { Source = source, Width = size, Height = size };
            }
        return new FontIcon { Glyph = "\uE77B", FontSize = size * 0.875, Width = size, Height = size, TextAlignment = TextAlignment.Center };
    }

    internal static FrameworkElement Provider(JsonElement provider, double size = 28)
    {
        var name = Json.Text(provider, "icon");
        var agy = name == "agy" || Json.OptionalText(provider, "preset_id") == "agy";
        if (!new[] { "aws", "baidu", "claude", "custom", "deepseek", "kiro", "openai", "opencode", "openrouter" }.Contains(name)) name = "custom";
        if ((Source(agy ? "agents/agy.png" : $"providers/{name}.svg") ?? Source("providers/custom.svg")) is { } source)
            return !agy && name is "openai" or "baidu" or "custom"
                ? ThemedMark(source, size)
                : new Image { Source = source, Width = size, Height = size };
        return new FontIcon { Glyph = "\uE753", FontSize = size * 0.8, Width = size, Height = size, TextAlignment = TextAlignment.Center };
    }

    private static bool IsMonochromeAgent(string key, string id)
    {
        if (!key.EndsWith(".svg", StringComparison.Ordinal)) return false;
        if (monochrome.TryGetValue(key, out var cached)) return cached;
        var path = Path.Combine(AppContext.BaseDirectory, "Assets", key);
        return monochrome[key] = id is "aside" or "openchamber"
            || File.ReadAllText(path).Contains("fill=\"currentColor\"", StringComparison.Ordinal);
    }

    private static FrameworkElement ThemedMark(ImageSource source, double size)
    {
        var mark = new Border { Width = size, Height = size, OpacityMask = new ImageBrush(source) { Stretch = Stretch.Uniform } };
        mark.SetResourceReference(Border.BackgroundProperty, "TextFillColorPrimaryBrush");
        return mark;
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
