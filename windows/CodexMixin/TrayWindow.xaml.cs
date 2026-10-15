using System.Text.Json;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace CodexMixin;

// XAML window roots are framework activation types.
public sealed partial class TrayWindow : Window
{
    private const int VisibleUsageModels = 20;
    private readonly CancellationTokenSource lifetime = new();
    private JsonElement[] providers = [];
    private string? providerId;
    private string days = "1";
    private bool busy;
    private bool settingState;
    private bool pinned;
    private bool exiting;
    private int maximumHeight;

    public TrayWindow()
    {
        InitializeComponent();
        Panel.RequestedTheme = ((App)Microsoft.UI.Xaml.Application.Current).Theme;
        Title = "Codex Mixin · 主面板";
        AppWindow.IsShownInSwitchers = false;
        var presenter = (OverlappedPresenter)AppWindow.Presenter;
        presenter.SetBorderAndTitleBar(false, false);
        presenter.IsResizable = false;
        presenter.IsMinimizable = false;
        presenter.IsMaximizable = false;
        presenter.IsAlwaysOnTop = true;
        Activated += (_, args) => { if (!pinned && !busy && args.WindowActivationState == WindowActivationState.Deactivated) AppWindow.Hide(); };
        Closed += (_, _) => lifetime.Cancel();
        AppWindow.Closing += (_, args) => { if (!exiting) { args.Cancel = true; AppWindow.Hide(); } };
    }

    internal void Shutdown() { exiting = true; Close(); }
    internal void HidePanel() => AppWindow.Hide();
    internal void SetTheme(ElementTheme theme) => Panel.RequestedTheme = theme;

    internal async void ShowPanel()
    {
        pinned = Environment.GetCommandLineArgs().Contains("--capture-ui");
        var bounds = NativeShell.PanelBounds();
        maximumHeight = bounds.Height;
        AppWindow.MoveAndResize(bounds);
        AppWindow.Show(); Activate();
        await ReloadAsync();
    }

    private async Task ReloadAsync()
    {
        if (busy) return;
        busy = true; GatewaySwitch.IsEnabled = false; PanelStatus.Text = "正在读取…";
        foreach (var range in UsageRanges.Children.Cast<Microsoft.UI.Xaml.Controls.Primitives.ToggleButton>())
            range.IsChecked = (string)range.Tag == days;
        try
        {
            var client = ((App)Microsoft.UI.Xaml.Application.Current).Client;
            var status = await client.QueryAsync(["service", "status", "--json"], lifetime.Token);
            ServiceTitle.Text = $"本地服务{Json.Gateway(status)}";
            var running = Json.OptionalText(status, "gateway") == "running";
            Endpoint.Text = Json.OptionalText(status, "endpoint") ?? (Json.Flag(status, "configured") ? "网关未启动" : "请先配置服务商");
            ServiceDot.Style = Themed(running ? "DotOn" : "DotOff");
            settingState = true; GatewaySwitch.IsOn = running; settingState = false;
            providers = Json.Flag(status, "configured")
                ? Json.Rows(Json.Field(await client.QueryAsync(["provider", "list", "--json"], lifetime.Token), "providers")) : [];
            if (!providers.Any(row => Json.Text(row, "id") == providerId))
                providerId = providers.Length == 0 ? null : Json.Text(providers[0], "id");
            ProviderStrip.Children.Clear();
            foreach (var provider in providers)
            {
                var id = Json.Text(provider, "id");
                var tile = new Border { Style = Themed(id == providerId ? "IconTile" : "IconTileIdle"), Width = 40, Height = 40, Child = Marks.Provider(provider, 24) };
                var button = new Button { Content = tile, Style = Themed("FlatButton"), Padding = new Thickness(0), CornerRadius = new CornerRadius(8) };
                ToolTipService.SetToolTip(button, Json.Text(provider, "display_name"));
                Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(button, Json.Text(provider, "display_name"));
                button.Click += async (_, _) => { providerId = id; await ReloadAsync(); };
                ProviderStrip.Children.Add(button);
            }
            var selected = providers.FirstOrDefault(row => Json.Text(row, "id") == providerId);
            var hasProvider = selected.ValueKind == JsonValueKind.Object;
            ProviderTitle.Text = hasProvider ? Json.Text(selected, "display_name") : "尚未配置服务商";
            ProviderIcon.Visibility = hasProvider ? Visibility.Visible : Visibility.Collapsed;
            if (hasProvider) ProviderIcon.Source = Marks.Provider(selected, 22).Source;
            UsageBody.Children.Clear(); QuotaBody.Children.Clear();
            if (Json.Flag(status, "configured"))
            {
                var quotas = Json.Rows(await client.QueryAsync(["quota", "--json"], lifetime.Token));
                foreach (var row in quotas.Where(row => Json.Text(row, "provider_id") == providerId)) RenderQuota(row);
            }
            if (QuotaBody.Children.Count == 0) QuotaBody.Children.Add(new TextBlock { Text = "额度未知", Style = Themed("Caption") });
            if (!running) UsageBody.Children.Add(Placeholder("本地网关未运行，暂无可读取的用量。"));
            else
            {
                var arguments = new List<string> { "usage", "--json" };
                if (days != "all") arguments.AddRange(["--days", days]);
                var rows = Json.Rows(await client.QueryAsync(arguments.ToArray(), lifetime.Token)).Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
                if (rows.Length > 0) RenderUsage(rows);
                if (rows.Length == 0) UsageBody.Children.Add(Placeholder("此时间范围暂无请求记录。"));
            }
            PanelStatus.Text = $"{DateTime.Now:HH:mm:ss} · 配置已同步";
        }
        catch (Exception error)
        {
            ServiceTitle.Text = "本地服务 · 状态未知";
            ServiceDot.Style = Themed("DotWarn");
            PanelStatus.Text = error.Message;
        }
        finally { busy = false; GatewaySwitch.IsEnabled = true; }
        FitToContent();
    }

    // Like the macOS menu, the panel is as tall as its content and keeps its bottom edge.
    private void FitToContent()
    {
        if (Panel.XamlRoot is not { } root || maximumHeight == 0) return;
        var scale = root.RasterizationScale;
        Panel.Measure(new Windows.Foundation.Size(AppWindow.Size.Width / scale, double.PositiveInfinity));
        var height = Math.Min(maximumHeight, (int)Math.Ceiling(Panel.DesiredSize.Height * scale));
        var bottom = AppWindow.Position.Y + AppWindow.Size.Height;
        AppWindow.MoveAndResize(new Windows.Graphics.RectInt32(AppWindow.Position.X, bottom - height, AppWindow.Size.Width, height));
    }

    private static Style Themed(string key) => (Style)Microsoft.UI.Xaml.Application.Current.Resources[key];

    private static TextBlock Placeholder(string text) => new()
    {
        Text = text, Style = Themed("Caption"), TextWrapping = TextWrapping.Wrap,
        HorizontalAlignment = HorizontalAlignment.Center, Margin = new Thickness(0, 24, 0, 24)
    };

    private static double? Amount(JsonElement row, string field)
    {
        if (!row.TryGetProperty(field, out var value) || value.ValueKind == JsonValueKind.Null) return null;
        if (value.ValueKind != JsonValueKind.Number || !value.TryGetDouble(out var number) || !double.IsFinite(number) || number < 0)
            throw new InvalidOperationException($"CLI 返回了无效的 {field} 数值。");
        return number;
    }

    private void RenderQuota(JsonElement row)
    {
        var used = Amount(row, "used") ?? Amount(row, "value");
        var limit = Amount(row, "limit");
        var remaining = Amount(row, "remaining");
        var currency = Json.OptionalText(row, "currency") ?? "";
        var label = Json.OptionalText(row, "label") ?? Json.OptionalText(row, "quota_id") ?? "额度";
        var value = used is { } consumed && limit is { } maximum ? $"{consumed:0.##} / {maximum:0.##} {currency}"
            : remaining is { } balance ? $"余额 {balance:0.##} {currency}" : Json.OptionalText(row, "error") ?? "额度未知";
        var heading = new Grid { ColumnSpacing = 8 };
        heading.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        heading.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        heading.Children.Add(new TextBlock { Text = label, FontSize = 13, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, TextTrimming = TextTrimming.CharacterEllipsis });
        var text = new TextBlock { Text = value, Style = Themed("MonoPrimary"), MaxWidth = 200 };
        ToolTipService.SetToolTip(text, value);
        Grid.SetColumn(text, 1); heading.Children.Add(text);
        var body = new StackPanel { Spacing = 6 }; body.Children.Add(heading);
        if (used is { } amount && limit is > 0)
            body.Children.Add(Bar(Math.Clamp(amount / limit.Value, 0, 1)));
        QuotaBody.Children.Add(body);
    }

    // Star columns size the fill without waiting for a layout pass.
    private static Grid Bar(double fraction)
    {
        var bar = new Grid { Height = 8 };
        bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(Math.Max(fraction, 0.0001), GridUnitType.Star) });
        bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(Math.Max(1 - fraction, 0.0001), GridUnitType.Star) });
        var track = new Border { Style = Themed("BarTrack") };
        Grid.SetColumnSpan(track, 2);
        bar.Children.Add(track);
        if (fraction > 0) bar.Children.Add(new Border { Style = Themed("BarFill") });
        return bar;
    }

    private static double TokenTotal(JsonElement row) => new[] { "input_tokens", "cache_read_tokens", "cache_creation_tokens", "output_tokens" }
        .Sum(field => Amount(row, field) ?? throw new InvalidOperationException($"CLI 缺少 {field}。"));

    private static string TokenLabel(double value) => value >= 1_000_000 ? $"{value / 1_000_000:0.#}M" : value >= 1000 ? $"{value / 1000:0.#}K" : $"{value:0}";

    private void RenderUsage(JsonElement[] rows)
    {
        var models = rows.Select(row => (Row: row, Total: TokenTotal(row))).OrderByDescending(row => row.Total).Take(VisibleUsageModels).ToArray();
        var maximum = Math.Max(1, models.Max(row => row.Total));
        const double plotHeight = 96;
        var chart = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4 };
        foreach (var (row, total) in models)
        {
            var model = Json.Text(row, "model_id");
            var column = new StackPanel { Spacing = 6, Width = 76 };
            var plot = new Grid { Height = plotHeight + 18 };
            var barHeight = Math.Max(4, total / maximum * plotHeight);
            plot.Children.Add(new TextBlock { Text = TokenLabel(total), Style = Themed("Caption"), HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Bottom, Margin = new Thickness(0, 0, 0, barHeight + 4) });
            plot.Children.Add(new Border { Style = Themed("BarFill"), Width = 12, Height = barHeight, VerticalAlignment = VerticalAlignment.Bottom, Opacity = 0.85 });
            column.Children.Add(plot);
            column.Children.Add(new TextBlock { Text = model, Style = Themed("Caption"), FontSize = 11, HorizontalAlignment = HorizontalAlignment.Center });
            ToolTipService.SetToolTip(column, $"{model}\n请求 {Json.Field(row, "request_count")} · 输入 {Json.Field(row, "input_tokens")} · 缓存输入 {Json.Field(row, "cache_read_tokens")} · 输出 {Json.Field(row, "output_tokens")}");
            chart.Children.Add(column);
        }
        UsageBody.Children.Add(new ScrollViewer { Content = chart, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto, VerticalScrollBarVisibility = ScrollBarVisibility.Disabled });
        if (rows.Length > VisibleUsageModels) UsageBody.Children.Add(new TextBlock { Text = $"展示用量最高的 {VisibleUsageModels} 个模型", Style = Themed("Caption"), FontSize = 11 });
    }

    private async void OnGatewayToggled(object sender, RoutedEventArgs args)
    {
        if (settingState || busy) return;
        await ChangeAsync(["service", GatewaySwitch.IsOn ? "start" : "stop", "--managed", "--json"]);
    }
    private async void OnRange(object sender, RoutedEventArgs args) { days = (string)((FrameworkElement)sender).Tag; await ReloadAsync(); }
    private void OnOpen(object sender, RoutedEventArgs args)
    {
        var target = (string)((FrameworkElement)sender).Tag;
        AppWindow.Hide(); ((App)Microsoft.UI.Xaml.Application.Current).OpenPage(target);
    }
    private async void OnAutostart(object sender, RoutedEventArgs args) => await ChangeAsync(["service", "autostart", "enable", "--json"]);
    private async void OnDoctor(object sender, RoutedEventArgs args) => await ChangeAsync(["doctor", "--quick", "--json"]);
    private async void OnQuit(object sender, RoutedEventArgs args)
    {
        if (busy) { PanelStatus.Text = "请等待当前操作完成后退出。"; return; }
        var restore = (string)((Button)sender).Tag == "restore";
        var dialog = new ContentDialog
        {
            Title = restore ? "退出并恢复原配置？" : "退出 Codex Mixin？",
            Content = restore ? "由 CLI 恢复指向本地网关的客户端配置，并停止网关，然后退出界面。" : "退出界面后，本地网关会继续按 CLI 的服务设置运行。",
            PrimaryButtonText = restore ? "恢复并退出" : "退出", CloseButtonText = "取消", XamlRoot = Panel.XamlRoot, RequestedTheme = Panel.ActualTheme
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
        busy = true;
        try
        {
            if (restore) await ((App)Microsoft.UI.Xaml.Application.Current).Operations.ChangeAsync(["service", "stop", "--managed", "--restore-clients", "--json"], lifetime.Token);
            ((App)Microsoft.UI.Xaml.Application.Current).ExitAll();
        }
        catch (Exception error) { PanelStatus.Text = $"退出失败：{error.Message}"; }
        finally { busy = false; }
    }
    private async Task ChangeAsync(string[] args)
    {
        if (busy) return;
        busy = true;
        try { PanelStatus.Text = await ((App)Microsoft.UI.Xaml.Application.Current).Operations.ChangeAsync(args, lifetime.Token); }
        catch (Exception error) { PanelStatus.Text = error.Message; }
        finally { busy = false; }
        await ReloadAsync();
    }
}
