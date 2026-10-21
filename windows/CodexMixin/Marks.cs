using System.Text.Json;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.UI.Xaml.Media;

namespace CodexMixin;

internal static class Marks
{
    // UI-thread-only; keys are the fixed set of packaged provider/agent icons.
    private static readonly Dictionary<string, ImageSource> sources = [];
    internal static FrameworkElement Agent(string id)
    {
        if (id.Length == 0 || id.Any(character => !char.IsAsciiLetterOrDigit(character) && character != '-'))
            throw new InvalidOperationException("CLI 返回了无效 Agent ID。");
        foreach (var extension in new[] { "svg", "png" })
        {
            var path = Path.Combine(AppContext.BaseDirectory, "Assets", "agents", $"{id}.{extension}");
            if (!File.Exists(path)) continue;
            var uri = new Uri($"ms-appx:///Assets/agents/{id}.{extension}");
            var key = $"agents/{id}.{extension}";
            if (!sources.TryGetValue(key, out var source))
                sources[key] = source = extension == "svg" ? new SvgImageSource(uri) : new BitmapImage(uri);
            return new Image { Source = source, Width = 32, Height = 32 };
        }
        return new SymbolIcon(Symbol.Contact) { Width = 32, Height = 32 };
    }

    internal static Image Provider(JsonElement provider, double size = 28)
    {
        var name = Json.Text(provider, "icon");
        if (!new[] { "aws", "baidu", "custom", "deepseek", "openai", "opencode", "openrouter" }.Contains(name)) name = "custom";
        var key = $"providers/{name}.svg";
        if (!sources.TryGetValue(key, out var source))
            sources[key] = source = new SvgImageSource(new Uri($"ms-appx:///Assets/{key}"));
        return new Image { Source = source, Width = size, Height = size };
    }
}
