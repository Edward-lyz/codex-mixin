using System.Text.Json;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;

namespace CodexMixin;

internal static class Marks
{
    internal static FrameworkElement Agent(string id)
    {
        if (id.Length == 0 || id.Any(character => !char.IsAsciiLetterOrDigit(character) && character != '-'))
            throw new InvalidOperationException("CLI 返回了无效 Agent ID。");
        foreach (var extension in new[] { "svg", "png" })
        {
            var path = Path.Combine(AppContext.BaseDirectory, "Assets", "agents", $"{id}.{extension}");
            if (!File.Exists(path)) continue;
            var uri = new Uri($"ms-appx:///Assets/agents/{id}.{extension}");
            return new Image { Source = extension == "svg" ? new SvgImageSource(uri) : new BitmapImage(uri), Width = 32, Height = 32 };
        }
        return new SymbolIcon(Symbol.Contact) { Width = 32, Height = 32 };
    }

    internal static Image Provider(JsonElement provider, double size = 28)
    {
        var name = Json.Text(provider, "icon");
        if (!new[] { "aws", "baidu", "custom", "deepseek", "openai", "opencode", "openrouter" }.Contains(name)) name = "custom";
        return new Image { Source = new SvgImageSource(new Uri($"ms-appx:///Assets/providers/{name}.svg")), Width = size, Height = size };
    }
}
