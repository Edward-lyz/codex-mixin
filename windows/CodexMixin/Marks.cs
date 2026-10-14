using System.Text.Json;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;

namespace CodexMixin;

internal static class Marks
{
    internal static Image Provider(JsonElement provider, double size = 28)
    {
        var name = Json.Text(provider, "icon");
        if (!new[] { "aws", "baidu", "custom", "deepseek", "openai", "opencode", "openrouter" }.Contains(name)) name = "custom";
        return new Image { Source = new SvgImageSource(new Uri($"ms-appx:///Assets/providers/{name}.svg")), Width = size, Height = size };
    }
}
