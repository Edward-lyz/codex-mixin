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
    private readonly CancellationTokenSource lifetime = new();
    private JsonElement[] providers = [];
    private string? providerId;
    private JsonElement[] quotaRows = [];
    private JsonElement[] allQuotaRows = [];
    private QuotaStyle quotaStyle = QuotaView.Stored();
    private bool busy;
    private bool settingState;
    private bool dialogOpen;
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
        Activated += (_, args) => { if (!dialogOpen && args.WindowActivationState == WindowActivationState.Deactivated) AppWindow.Hide(); };
        Closed += (_, _) => lifetime.Cancel();
        AppWindow.Closing += (_, args) => { if (!exiting) { args.Cancel = true; AppWindow.Hide(); } };
    }

    internal void Shutdown() { exiting = true; Close(); }
    internal void HidePanel() => AppWindow.Hide();
    internal void SetTheme(ElementTheme theme) => Panel.RequestedTheme = theme;

    internal async void ShowPanel()
    {
        var bounds = NativeShell.PanelBounds();
        maximumHeight = bounds.Height;
        AppWindow.MoveAndResize(bounds);
        // The console may have changed the style since this cached panel last showed.
        quotaStyle = QuotaView.Stored();
        RenderQuota();
        AppWindow.Show(); Activate();
        await ReloadAsync();
    }

    private async Task ReloadAsync()
    {
        if (busy) return;
        busy = true; GatewaySwitch.IsEnabled = false; PanelStatus.Text = "正在读取…";
        try
        {
            var client = ((App)Microsoft.UI.Xaml.Application.Current).Client;
            // Quota may use the network; render local status/providers independently.
            using var refresh = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
            var quotaTask = client.QueryAsync(["quota", "--json"], refresh.Token);
            var statusTask = client.QueryAsync(["service", "status", "--json"], refresh.Token);
            var providersTask = client.QueryAsync(["provider", "list", "--json"], refresh.Token);
            // Observe every subprocess even when one fails.
            var snapshot = Task.WhenAll(statusTask, providersTask, quotaTask);
            try
            {
                var status = await statusTask;
                ServiceTitle.Text = $"本地服务{Json.Gateway(status)}";
                var running = Json.OptionalText(status, "gateway") == "running";
                Endpoint.Text = Json.OptionalText(status, "endpoint") ?? (Json.Flag(status, "configured") ? "网关未启动" : "请先配置服务商");
                ServiceDot.Style = Themed(running ? "DotOn" : "DotOff");
                settingState = true; GatewaySwitch.IsOn = running; settingState = false;
                providers = Json.Flag(status, "configured")
                    ? Json.Rows(Json.Field(await providersTask, "providers")) : [];
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
                    button.Click += (_, _) =>
                    {
                        providerId = id;
                        foreach (var item in ProviderStrip.Children.OfType<Button>())
                            ((Border)item.Content).Style = Themed(ReferenceEquals(item, button) ? "IconTile" : "IconTileIdle");
                        quotaRows = allQuotaRows.Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
                        RenderQuota();
                        FitToContent();
                    };
                    ProviderStrip.Children.Add(button);
                }
                var selected = providers.FirstOrDefault(row => Json.Text(row, "id") == providerId);
                var hasProvider = selected.ValueKind == JsonValueKind.Object;
                ProviderTitle.Text = hasProvider ? "额度" : "尚未配置服务商";
                allQuotaRows = Json.Rows(await quotaTask);
                quotaRows = allQuotaRows.Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
                RenderQuota();
                await snapshot;
                PanelStatus.Text = $"{DateTime.Now:HH:mm:ss} · 配置已同步";
            }
            catch
            {
                refresh.Cancel();
                try { await snapshot; }
                catch (Exception) { /* Observe sibling failures; propagate the original error below. */ }
                throw;
            }
        }
        catch (Exception error)
        {
            allQuotaRows = quotaRows = [];
            RenderQuota();
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

    private void RenderQuota()
    {
        QuotaBody.Children.Clear();
        QuotaStyleHost.Child = quotaRows.Length == 0 ? null : QuotaView.StyleSwitch(quotaStyle, style =>
        {
            quotaStyle = style;
            RenderQuota();
            FitToContent();
        });
        QuotaBody.Children.Add(quotaRows.Length == 0
            ? new TextBlock { Text = "额度未知", Style = Themed("Caption") }
            : QuotaView.Render(quotaRows, quotaStyle));
    }

    private async void OnGatewayToggled(object sender, RoutedEventArgs args)
    {
        if (settingState || busy) return;
        await ChangeAsync(["service", GatewaySwitch.IsOn ? "start" : "stop", "--managed", "--json"]);
    }
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
        var dialog = new ContentDialog
        {
            Title = "退出 Codex Mixin？",
            Content = "退出界面后，本地网关会继续按 CLI 的服务设置运行。",
            PrimaryButtonText = "退出", CloseButtonText = "取消", XamlRoot = Panel.XamlRoot, RequestedTheme = Panel.ActualTheme
        };
        dialogOpen = true;
        try
        {
            if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
            ((App)Microsoft.UI.Xaml.Application.Current).ExitAll();
        }
        finally { dialogOpen = false; }
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
