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
    private readonly Dictionary<string, DateTimeOffset> quotaUpdatedAt = [];
    private DateTimeOffset refreshedAt;
    private long snapshotRevision;
    private static readonly TimeSpan SnapshotAge = TimeSpan.FromSeconds(30);
    private QuotaStyle quotaStyle = QuotaView.Stored();
    private bool busy;
    private bool changing;
    private bool settingState;
    private bool serviceKnown;
    private bool dialogOpen;
    private bool exiting;
    private int maximumHeight;
    private bool fitting;
    private bool fitQueued;
    private CancellationTokenSource? refreshCancellation;
    private Task refreshTask = Task.CompletedTask;
    private static readonly TimeSpan LocalReadTimeout = TimeSpan.FromSeconds(10);
    private static readonly TimeSpan QuotaReadTimeout = TimeSpan.FromSeconds(20);

    public TrayWindow()
    {
        InitializeComponent();
        GatewaySwitch.IsEnabled = false;
        Panel.RequestedTheme = ((App)Microsoft.UI.Xaml.Application.Current).Theme;
        Title = "Codex Mixin · 主面板";
        AppWindow.IsShownInSwitchers = false;
        var presenter = (OverlappedPresenter)AppWindow.Presenter;
        presenter.SetBorderAndTitleBar(false, false);
        presenter.IsResizable = false;
        presenter.IsMinimizable = false;
        presenter.IsMaximizable = false;
        presenter.IsAlwaysOnTop = true;
        NativeShell.ConfigurePopupFrame(WinRT.Interop.WindowNative.GetWindowHandle(this));
        Activated += (_, args) => { if (!dialogOpen && args.WindowActivationState == WindowActivationState.Deactivated) HidePanel(); };
        Panel.Loaded += (_, _) => FitToContent();
        PanelContent.SizeChanged += (_, _) => FitToContent();
        PanelMenu.SizeChanged += (_, _) => FitToContent();
        Closed += (_, _) => lifetime.Cancel();
        AppWindow.Closing += (_, args) => { if (!exiting) { args.Cancel = true; HidePanel(); } };
    }

    internal bool IsChanging => changing;
    internal void Shutdown() { exiting = true; Close(); }
    internal void HidePanel()
    {
        refreshCancellation?.Cancel();
        AppWindow.Hide();
    }
    internal void SetTheme(ElementTheme theme) => Panel.RequestedTheme = theme;
    internal async Task PreloadAsync()
    {
        // Reuse the same bounded reads before the user's first tray click.
        await ReloadAsync();
    }

    internal async void ShowPanel()
    {
        var bounds = NativeShell.PanelBounds();
        maximumHeight = bounds.Height;
        // Start compact even before XAML has a root; remote reads never size the window.
        var initialHeight = Math.Min(bounds.Height, bounds.Width);
        bounds.Y += bounds.Height - initialHeight;
        bounds.Height = initialHeight;
        AppWindow.MoveAndResize(bounds);
        // The console may have changed the style since this cached panel last showed.
        quotaStyle = QuotaView.Stored();
        RenderQuota();
        AppWindow.Show(); Activate();
        NativeShell.ConfigurePopupFrame(WinRT.Interop.WindowNative.GetWindowHandle(this));
        FitToContent();
        // A previous hide may still be reaping its cancelled query processes.
        await refreshTask;
        if (exiting || changing || !AppWindow.IsVisible) return;
        if (DateTimeOffset.UtcNow - refreshedAt < SnapshotAge && snapshotRevision == ((App)Microsoft.UI.Xaml.Application.Current).Operations.Revision)
        {
            RenderQuota();
            return;
        }
        await ReloadAsync();
    }

    private Task ReloadAsync()
    {
        if (busy || changing || exiting) return refreshTask;
        refreshTask = RefreshAsync();
        return refreshTask;
    }

    private async Task RefreshAsync()
    {
        busy = true;
        GatewaySwitch.IsEnabled = false;
        PanelStatus.Text = "正在刷新本地状态与额度…";
        using var refresh = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
        refreshCancellation = refresh;
        var revision = ((App)Microsoft.UI.Xaml.Application.Current).Operations.Revision;
        RenderQuota();
        try
        {
            var client = ((App)Microsoft.UI.Xaml.Application.Current).Client;
            // Reserve both query slots for local reads first. Each section renders and
            // fails independently; quota must never disable an already-known gateway.
            var statusTask = StatusAsync();
            var providersTask = ProvidersAsync();
            var errors = await Task.WhenAll(statusTask, providersTask, QuotaAsync());
            if (refresh.IsCancellationRequested) return;
            var failures = errors.OfType<string>().ToArray();
            PanelStatus.Text = failures.Length == 0
                ? $"{DateTime.Now:HH:mm:ss} · 配置已同步"
                : string.Join("\n", failures);
            if (failures.Length == 0)
            {
                refreshedAt = DateTimeOffset.UtcNow;
                snapshotRevision = revision;
            }

            async Task<string?> StatusAsync()
            {
                try
                {
                    var status = await client.QueryAsync(["service", "status", "--json"], refresh.Token, LocalReadTimeout);
                    refresh.Token.ThrowIfCancellationRequested();
                    ServiceTitle.Text = $"本地服务{Json.Gateway(status)}";
                    serviceKnown = true;
                    var running = Json.OptionalText(status, "gateway") == "running";
                    Endpoint.Text = Json.OptionalText(status, "endpoint") ?? (Json.Flag(status, "configured") ? "网关未启动" : "请先配置服务商");
                    ServiceDot.Style = Themed(running ? "DotOn" : "DotOff");
                    settingState = true; GatewaySwitch.IsOn = running; settingState = false;
                    GatewaySwitch.IsEnabled = !changing;
                    FitToContent();
                    return null;
                }
                catch (Exception error)
                {
                    if (refresh.IsCancellationRequested) return null;
                    serviceKnown = false;
                    GatewaySwitch.IsEnabled = false;
                    ServiceTitle.Text = "本地服务 · 状态未知";
                    Endpoint.Text = "请重新打开面板重试";
                    ServiceDot.Style = Themed("DotWarn");
                    return $"状态读取失败：{error.Message}";
                }
            }

            async Task<string?> ProvidersAsync()
            {
                try
                {
                    var snapshot = await client.QueryAsync(["provider", "list", "--json"], refresh.Token, LocalReadTimeout);
                    refresh.Token.ThrowIfCancellationRequested();
                    providers = Json.Rows(Json.Field(snapshot, "providers"));
                    var ids = providers.Select(row => Json.Text(row, "id")).ToHashSet(StringComparer.Ordinal);
                    allQuotaRows = allQuotaRows.Where(row => ids.Contains(Json.Text(row, "provider_id"))).ToArray();
                    foreach (var removed in quotaUpdatedAt.Keys.Where(id => !ids.Contains(id)).ToArray()) quotaUpdatedAt.Remove(removed);
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
                        button.Click += async (_, _) =>
                        {
                            providerId = id;
                            foreach (var item in ProviderStrip.Children.OfType<Button>())
                                ((Border)item.Content).Style = Themed(ReferenceEquals(item, button) ? "IconTile" : "IconTileIdle");
                            quotaRows = allQuotaRows.Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
                            RenderQuota();
                            FitToContent();
                            // The CLI's provider filter avoids waiting for unrelated
                            // accounts. Retain each provider's last measured display.
                            refreshCancellation?.Cancel();
                            await refreshTask;
                            if (!exiting && !changing && AppWindow.IsVisible) await ReloadAsync();
                        };
                        ProviderStrip.Children.Add(button);
                    }
                    var selected = providers.FirstOrDefault(row => Json.Text(row, "id") == providerId);
                    var hasProvider = selected.ValueKind == JsonValueKind.Object;
                    ProviderTitle.Text = hasProvider ? "额度" : "尚未配置服务商";
                    quotaRows = allQuotaRows.Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
                    RenderQuota();
                    FitToContent();
                    return null;
                }
                catch (Exception error)
                {
                    if (refresh.IsCancellationRequested) return null;
                    providers = [];
                    ProviderStrip.Children.Clear();
                    ProviderTitle.Text = "服务商读取失败";
                    return $"服务商读取失败：{error.Message}";
                }
            }

            async Task<string?> QuotaAsync()
            {
                try
                {
                    if (await providersTask != null || providerId is not { } selectedId) return null;
                    var snapshot = await client.QueryAsync(["quota", "--provider", selectedId, "--json"], refresh.Token, QuotaReadTimeout);
                    refresh.Token.ThrowIfCancellationRequested();
                    var rows = Json.Rows(snapshot);
                    var quotaError = rows.Select(row => Json.OptionalText(row, "error")).FirstOrDefault(error => error != null);
                    if (quotaError != null && quotaUpdatedAt.ContainsKey(selectedId))
                        return $"额度刷新失败，显示上次数据：{quotaError}";
                    allQuotaRows = allQuotaRows.Where(row => Json.Text(row, "provider_id") != selectedId).Concat(rows).ToArray();
                    if (quotaError == null && rows.Length > 0) quotaUpdatedAt[selectedId] = DateTimeOffset.UtcNow;
                    quotaRows = allQuotaRows.Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
                    RenderQuota();
                    FitToContent();
                    return quotaError == null ? null : $"额度读取失败：{quotaError}";
                }
                catch (Exception error)
                {
                    if (refresh.IsCancellationRequested) return null;
                    return $"额度读取失败：{error.Message}";
                }
            }
        }
        catch (Exception error)
        {
            allQuotaRows = quotaRows = [];
            RenderQuota();
            serviceKnown = false;
            ServiceTitle.Text = "本地服务 · 状态未知";
            ServiceDot.Style = Themed("DotWarn");
            PanelStatus.Text = error.Message;
        }
        finally
        {
            refreshCancellation = null;
            busy = false;
            if (!exiting)
            {
                GatewaySwitch.IsEnabled = serviceKnown && !changing;
                RenderQuota();
            }
        }
        FitToContent();
    }

    // Like the macOS menu, the panel is as tall as its content and keeps its bottom edge.
    private void FitToContent()
    {
        if (exiting || fitting || fitQueued) return;
        fitQueued = DispatcherQueue.TryEnqueue(() =>
        {
            fitQueued = false;
            FitPanelToContent();
        });
    }

    private void FitPanelToContent()
    {
        if (exiting || fitting || !Panel.IsLoaded || Panel.XamlRoot is not { } root || maximumHeight == 0) return;
        fitting = true;
        try
        {
            var scale = root.RasterizationScale;
            var width = AppWindow.Size.Width / scale - 2;
            // Measuring the Window's root re-enters WinUI layout and can throw inside
            // ScrollViewer before its visual tree is ready. Measure only the content.
            PanelContent.Measure(new Windows.Foundation.Size(width, double.PositiveInfinity));
            PanelMenu.Measure(new Windows.Foundation.Size(width - 12, double.PositiveInfinity));
            var footer = PanelMenu.DesiredSize.Height + 14;
            PanelScroll.MaxHeight = Math.Max(1, maximumHeight / scale - footer);
            var height = Math.Min(maximumHeight, (int)Math.Ceiling((Math.Min(PanelContent.DesiredSize.Height, PanelScroll.MaxHeight) + footer) * scale));
            var bottom = AppWindow.Position.Y + AppWindow.Size.Height;
            if (height != AppWindow.Size.Height)
                AppWindow.MoveAndResize(new Windows.Graphics.RectInt32(AppWindow.Position.X, bottom - height, AppWindow.Size.Width, height));
        }
        finally { fitting = false; }
    }

    private static Style Themed(string key) => (Style)Microsoft.UI.Xaml.Application.Current.Resources[key];

    private void RenderQuota()
    {
        QuotaBody.Children.Clear();
        QuotaStamp.Text = providerId is { } id && quotaUpdatedAt.TryGetValue(id, out var updatedAt)
            ? $"上次更新 {updatedAt.ToLocalTime():HH:mm:ss}{(busy ? " · 正在刷新" : "")}" : "";
        QuotaStyleHost.Child = quotaRows.Length == 0 ? null : QuotaView.StyleSwitch(quotaStyle, style =>
        {
            quotaStyle = style;
            RenderQuota();
            FitToContent();
        });
        if (quotaRows.Length == 0 && busy)
        {
            QuotaBody.Children.Add(new ProgressBar { IsIndeterminate = AppWindow.IsVisible, Height = 4 });
            QuotaBody.Children.Add(new TextBlock { Text = "正在读取额度…", Style = Themed("Caption") });
        }
        else QuotaBody.Children.Add(quotaRows.Length == 0
            ? new TextBlock { Text = "额度未知", Style = Themed("Caption") }
            : QuotaView.Render(quotaRows, quotaStyle));
    }

    private async void OnGatewayToggled(object sender, RoutedEventArgs args)
    {
        if (settingState || changing || !serviceKnown) return;
        await ChangeAsync(["service", GatewaySwitch.IsOn ? "start" : "stop", "--managed", "--json"]);
    }
    private void OnOpen(object sender, RoutedEventArgs args)
    {
        var target = (string)((FrameworkElement)sender).Tag;
        HidePanel(); ((App)Microsoft.UI.Xaml.Application.Current).OpenPage(target);
    }
    private async void OnAutostart(object sender, RoutedEventArgs args) => await ChangeAsync(["service", "autostart", "enable", "--json"]);
    private async void OnRefreshQuota(object sender, RoutedEventArgs args)
    {
        if (changing) return;
        refreshCancellation?.Cancel();
        await refreshTask;
        if (!exiting && AppWindow.IsVisible) await ReloadAsync();
    }
    private async void OnDoctor(object sender, RoutedEventArgs args) => await ChangeAsync(["doctor", "--quick", "--json"]);
    private async void OnQuit(object sender, RoutedEventArgs args)
    {
        if (((App)Microsoft.UI.Xaml.Application.Current).HasPendingChanges) { PanelStatus.Text = "请等待当前操作完成后退出。"; return; }
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
        if (changing) return;
        changing = true;
        GatewaySwitch.IsEnabled = false;
        refreshCancellation?.Cancel();
        await refreshTask;
        string? failure = null;
        try
        {
            PanelStatus.Text = "正在执行网关操作…";
            FitToContent();
            PanelStatus.Text = await ((App)Microsoft.UI.Xaml.Application.Current).Operations.ChangeAsync(args, lifetime.Token);
        }
        catch (Exception error) { failure = error.Message; }
        finally { changing = false; }
        if (!exiting && AppWindow.IsVisible) await ReloadAsync();
        if (!exiting && failure != null) { PanelStatus.Text = failure; FitToContent(); }
    }
}
