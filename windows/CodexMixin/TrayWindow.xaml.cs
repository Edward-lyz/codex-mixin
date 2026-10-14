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
        AppWindow.MoveAndResize(NativeShell.PanelBounds());
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
            Endpoint.Text = Json.OptionalText(status, "endpoint") ?? "请先配置服务商";
            var running = Json.OptionalText(status, "gateway") == "running";
            ServiceDot.Fill = new SolidColorBrush(running ? Colors.LimeGreen : Colors.Gray);
            settingState = true; GatewaySwitch.IsOn = running; settingState = false;
            providers = Json.Flag(status, "configured")
                ? Json.Rows(Json.Field(await client.QueryAsync(["provider", "list", "--json"], lifetime.Token), "providers")) : [];
            if (!providers.Any(row => Json.Text(row, "id") == providerId))
                providerId = providers.Length == 0 ? null : Json.Text(providers[0], "id");
            ProviderStrip.Children.Clear();
            foreach (var provider in providers)
            {
                var id = Json.Text(provider, "id");
                var button = new Button { Content = Marks.Provider(provider, 24), Padding = new Thickness(8), BorderBrush = (Brush)Microsoft.UI.Xaml.Application.Current.Resources[id == providerId ? "AccentFillColorDefaultBrush" : "CardStrokeColorDefaultBrush"] };
                ToolTipService.SetToolTip(button, Json.Text(provider, "display_name"));
                button.Click += async (_, _) => { providerId = id; await ReloadAsync(); };
                ProviderStrip.Children.Add(button);
            }
            var selected = providers.FirstOrDefault(row => Json.Text(row, "id") == providerId);
            ProviderTitle.Text = selected.ValueKind == JsonValueKind.Object ? Json.Text(selected, "display_name") : "尚未配置服务商";
            UsageBody.Children.Clear(); QuotaBody.Children.Clear();
            if (Json.Flag(status, "configured"))
            {
                var quotas = Json.Rows(await client.QueryAsync(["quota", "--json"], lifetime.Token));
                foreach (var row in quotas.Where(row => Json.Text(row, "provider_id") == providerId)) RenderQuota(row);
            }
            if (QuotaBody.Children.Count == 0) QuotaBody.Children.Add(new TextBlock { Text = "额度未知", FontSize = 12 });
            if (!running) UsageBody.Children.Add(new TextBlock { Text = "本地网关未运行，暂无可读取的用量。", TextWrapping = TextWrapping.Wrap, FontSize = 12 });
            else
            {
                var arguments = new List<string> { "usage", "--json" };
                if (days != "all") arguments.AddRange(["--days", days]);
                var rows = Json.Rows(await client.QueryAsync(arguments.ToArray(), lifetime.Token)).Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
                if (rows.Length > 0) RenderUsage(rows);
                if (rows.Length == 0) UsageBody.Children.Add(new TextBlock { Text = "此时间范围暂无请求记录。" });
            }
            PanelStatus.Text = $"{DateTime.Now:HH:mm:ss} · 配置已同步";
        }
        catch (Exception error)
        {
            ServiceTitle.Text = "本地服务 · 状态未知";
            ServiceDot.Fill = new SolidColorBrush(Colors.Orange);
            PanelStatus.Text = error.Message;
        }
        finally { busy = false; GatewaySwitch.IsEnabled = true; }
    }

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
        heading.Children.Add(new TextBlock { Text = label, FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        var text = new TextBlock { Text = value, FontSize = 12, MaxWidth = 180, TextTrimming = TextTrimming.CharacterEllipsis };
        ToolTipService.SetToolTip(text, value);
        Grid.SetColumn(text, 1); heading.Children.Add(text);
        var body = new StackPanel { Spacing = 6 }; body.Children.Add(heading);
        if (used is { } amount && limit is > 0)
            body.Children.Add(new ProgressBar { Minimum = 0, Maximum = 100, Value = Math.Clamp(amount / limit.Value * 100, 0, 100), Height = 7 });
        QuotaBody.Children.Add(body);
    }

    private static double TokenTotal(JsonElement row) => new[] { "input_tokens", "cache_read_tokens", "cache_creation_tokens", "output_tokens" }
        .Sum(field => Amount(row, field) ?? throw new InvalidOperationException($"CLI 缺少 {field}。"));

    private static string TokenLabel(double value) => value >= 1_000_000 ? $"{value / 1_000_000:0.#}M" : value >= 1000 ? $"{value / 1000:0.#}K" : $"{value:0}";

    private void RenderUsage(JsonElement[] rows)
    {
        var models = rows.Select(row => (Row: row, Total: TokenTotal(row))).OrderByDescending(row => row.Total).Take(VisibleUsageModels).ToArray();
        var maximum = Math.Max(1, models.Max(row => row.Total));
        var chart = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        foreach (var (row, total) in models)
        {
            var model = Json.Text(row, "model_id");
            var column = new StackPanel { Spacing = 6, Width = 72 };
            column.Children.Add(new TextBlock { Text = TokenLabel(total), FontSize = 11, HorizontalAlignment = HorizontalAlignment.Center });
            var plot = new Grid { Height = 110 };
            plot.Children.Add(new Border { Width = 8, Height = total / maximum * 110, CornerRadius = new CornerRadius(4), Background = (Brush)Microsoft.UI.Xaml.Application.Current.Resources["AccentFillColorDefaultBrush"], VerticalAlignment = VerticalAlignment.Bottom });
            column.Children.Add(plot);
            column.Children.Add(new TextBlock { Text = model, FontSize = 11, TextTrimming = TextTrimming.CharacterEllipsis });
            ToolTipService.SetToolTip(column, $"{model}\n请求 {Json.Field(row, "request_count")} · 输入 {Json.Field(row, "input_tokens")} · 缓存输入 {Json.Field(row, "cache_read_tokens")} · 输出 {Json.Field(row, "output_tokens")}");
            chart.Children.Add(column);
        }
        UsageBody.Children.Add(new ScrollViewer { Content = chart, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto, VerticalScrollBarVisibility = ScrollBarVisibility.Disabled });
        if (rows.Length > VisibleUsageModels) UsageBody.Children.Add(new TextBlock { Text = $"展示用量最高的 {VisibleUsageModels} 个模型", FontSize = 11 });
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
