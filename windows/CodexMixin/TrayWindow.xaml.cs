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
    private static readonly TimeSpan SnapshotAge = TimeSpan.FromSeconds(10);
    private readonly DispatcherTimer timer = new() { Interval = SnapshotAge };
    private readonly string cachePath = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMixin",
        Environment.GetCommandLineArgs().Contains("--capture-ui") ? "quota-display-ui-test.json" : "quota-display.json");
    private QuotaStyle quotaStyle = QuotaView.Stored();
    private bool busy;
    private bool preloadComplete;
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
    private Task statusRefreshTask = Task.CompletedTask;
    private CancellationTokenSource? statusCancellation;
    private readonly HashSet<string> quotaPending = [];
    private readonly Dictionary<string, string> quotaFailures = [];
    private string? quotaProvider;
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
        Closed += (_, _) => { timer.Stop(); lifetime.Cancel(); };
        timer.Tick += async (_, _) => await ReloadAsync();
        AppWindow.Closing += (_, args) => { if (!exiting) { args.Cancel = true; HidePanel(); } };
    }

    internal bool IsChanging => changing;
    internal void Shutdown() { exiting = true; Close(); }
    internal void HidePanel()
    {
        // macOS keeps quota warm while its menu is closed. Visibility must not
        // cancel a slow provider and force the next click to start over.
        AppWindow.Hide();
        RenderQuota();
    }
    internal void SetTheme(ElementTheme theme) => Panel.RequestedTheme = theme;
    internal async Task PreloadAsync()
    {
        try
        {
            if (await QuotaDisplayCache.LoadAsync(cachePath, lifetime.Token) is { } cached)
            {
                providers = cached.Providers;
                allQuotaRows = cached.Rows;
                foreach (var pair in cached.UpdatedAt) quotaUpdatedAt[pair.Key] = pair.Value;
                providerId ??= providers.FirstOrDefault().ValueKind == JsonValueKind.Object ? Json.Text(providers[0], "id") : null;
                RenderProviders();
            }
        }
        catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { return; }
        catch (Exception error) { PanelStatus.Text = $"上次额度缓存不可用：{error.Message}"; }
        if (exiting) return;
        preloadComplete = true;
        timer.Start();
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
        // Paint cached values immediately; an existing background refresh is
        // reused without cancellation, even after switching providers.
        if (exiting || changing || !preloadComplete || !AppWindow.IsVisible) return;
        if (DateTimeOffset.UtcNow - refreshedAt < SnapshotAge && snapshotRevision == ((App)Microsoft.UI.Xaml.Application.Current).Operations.Revision)
        {
            RenderQuota();
            return;
        }
        await ReloadAsync();
    }

    private Task ReloadAsync()
    {
        if (changing || exiting) return refreshTask;
        if (busy)
        {
            if (statusRefreshTask.IsCompleted) statusRefreshTask = RefreshStatusOnlyAsync();
            return statusRefreshTask;
        }
        if (!statusRefreshTask.IsCompleted) return statusRefreshTask;
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
            var statusTask = ReadStatusAsync(refresh.Token);
            statusRefreshTask = statusTask;
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
                    RenderProviders();
                    return null;
                }
                catch (Exception error)
                {
                    if (refresh.IsCancellationRequested) return null;
                    ProviderTitle.Text = "服务商读取失败 · 保留上次显示";
                    return $"服务商读取失败：{error.Message}";
                }
            }

            async Task<string?> QuotaAsync()
            {
                try
                {
                    if (await providersTask != null)
                    {
                        if (providerId != null) quotaFailures[providerId] = "服务商读取失败，未查询额度。请刷新重试。";
                        return null;
                    }
                    var errors = new List<string>();
                    quotaPending.UnionWith(providers.Where(row => Json.Flag(row, "enabled")).Select(row => Json.Text(row, "id")));
                    RenderQuota();
                    // Selected provider first; each result updates independently.
                    // Only one remote subprocess is active, with local reads in
                    // separate CLI slots so quota cannot hold up navigation.
                    foreach (var provider in providers.Where(row => Json.Flag(row, "enabled")).OrderByDescending(row => Json.Text(row, "id") == providerId))
                    {
                        var selectedId = Json.Text(provider, "id");
                        quotaProvider = selectedId;
                        RenderQuota();
                        try
                        {
                            var snapshot = await client.QueryAsync(["quota", "--provider", selectedId, "--json"], refresh.Token, QuotaReadTimeout);
                            refresh.Token.ThrowIfCancellationRequested();
                            var rows = Json.Rows(snapshot);
                            var quotaError = rows.Select(row => Json.OptionalText(row, "error")).FirstOrDefault(error => error != null);
                            if (quotaError == null || !quotaUpdatedAt.ContainsKey(selectedId))
                                allQuotaRows = allQuotaRows.Where(row => Json.Text(row, "provider_id") != selectedId).Concat(rows).ToArray();
                            if (quotaError == null && rows.Length > 0) quotaUpdatedAt[selectedId] = DateTimeOffset.UtcNow;
                            if (quotaError != null)
                            {
                                quotaFailures[selectedId] = quotaError;
                                errors.Add($"{Json.Text(provider, "display_name")}：{quotaError}");
                            }
                            else quotaFailures.Remove(selectedId);
                        }
                        catch (Exception error)
                        {
                            if (refresh.IsCancellationRequested) return null;
                            quotaFailures[selectedId] = error.Message;
                            errors.Add($"{Json.Text(provider, "display_name")}：{error.Message}");
                        }
                        finally { quotaPending.Remove(selectedId); }
                        quotaRows = allQuotaRows.Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
                        RenderQuota();
                        FitToContent();
                    }
                    return errors.Count == 0 ? null : $"额度刷新失败（保留上次数据）：{string.Join("；", errors)}";
                }
                catch (Exception error)
                {
                    if (refresh.IsCancellationRequested) return null;
                    return $"额度读取失败：{error.Message}";
                }
                finally { quotaPending.Clear(); quotaProvider = null; RenderQuota(); }
            }
        }
        catch (Exception error)
        {
            RenderQuota();
            serviceKnown = false;
            ServiceTitle.Text = "本地服务 · 状态未知";
            ServiceDot.Style = Themed("DotWarn");
            PanelStatus.Text = error.Message;
        }
        finally
        {
            try
            {
                if (!exiting && quotaUpdatedAt.Count > 0)
                    await QuotaDisplayCache.Create(providers, allQuotaRows, quotaUpdatedAt).SaveAsync(cachePath, lifetime.Token);
            }
            catch (Exception error) when (!exiting) { PanelStatus.Text += $"\n额度展示缓存保存失败：{error.Message}"; }
            catch (OperationCanceledException) when (exiting) { }
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

    private async Task RefreshStatusOnlyAsync()
    {
        using var cancellation = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token, refreshCancellation?.Token ?? lifetime.Token);
        statusCancellation = cancellation;
        try
        {
            if (await ReadStatusAsync(cancellation.Token) is { } error && !cancellation.IsCancellationRequested)
                PanelStatus.Text = error;
        }
        finally { statusCancellation = null; }
    }

    private async Task<string?> ReadStatusAsync(CancellationToken token)
    {
        try
        {
            var status = await ((App)Microsoft.UI.Xaml.Application.Current).Client.QueryAsync(["service", "status", "--json"], token, LocalReadTimeout);
            token.ThrowIfCancellationRequested();
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
            if (token.IsCancellationRequested) return null;
            serviceKnown = false;
            GatewaySwitch.IsEnabled = false;
            ServiceTitle.Text = "本地服务 · 状态未知";
            Endpoint.Text = "请重新打开面板重试";
            ServiceDot.Style = Themed("DotWarn");
            return $"状态读取失败：{error.Message}";
        }
    }

    private void RenderProviders()
    {
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
                // Every provider is prefetched, so switching is a display action.
                if (!busy && !changing && !exiting &&
                    (!quotaUpdatedAt.TryGetValue(id, out var updatedAt) || DateTimeOffset.UtcNow - updatedAt >= SnapshotAge))
                    _ = ReloadAsync();
            };
            ProviderStrip.Children.Add(button);
        }
        var selected = providers.FirstOrDefault(row => Json.Text(row, "id") == providerId);
        var hasProvider = selected.ValueKind == JsonValueKind.Object;
        ProviderTitle.Text = hasProvider ? "额度" : "尚未配置服务商";
        quotaRows = allQuotaRows.Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
        RenderQuota();
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
            ? $"上次更新 {updatedAt.ToLocalTime():HH:mm:ss}{(quotaPending.Contains(id) ? " · 正在刷新" : "")}" : "";
        QuotaStyleHost.Child = quotaRows.Length == 0 ? null : QuotaView.StyleSwitch(quotaStyle, style =>
        {
            quotaStyle = style;
            RenderQuota();
            FitToContent();
        });
        if (quotaRows.Length == 0 && providerId != null && quotaPending.Contains(providerId))
        {
            QuotaBody.Children.Add(new ProgressBar { IsIndeterminate = AppWindow.IsVisible, Height = 4 });
            QuotaBody.Children.Add(new TextBlock { Text = quotaProvider == providerId ? "正在读取额度…" : "正在排队查询额度…", Style = Themed("Caption") });
        }
        else QuotaBody.Children.Add(quotaRows.Length == 0
            ? new TextBlock { Text = providerId != null && quotaFailures.TryGetValue(providerId, out var error) ? $"额度读取失败：{error}" : "额度未知", TextWrapping = TextWrapping.Wrap, Style = Themed("Caption") }
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
        statusCancellation?.Cancel();
        await Task.WhenAll(refreshTask, statusRefreshTask);
        if (!exiting && AppWindow.IsVisible) await ReloadAsync();
    }
    private async void OnDoctor(object sender, RoutedEventArgs args) => await ChangeAsync(["doctor", "--quick", "--json"]);
    private async void OnQuit(object sender, RoutedEventArgs args)
    {
        if (((App)Microsoft.UI.Xaml.Application.Current).HasPendingChanges) { PanelStatus.Text = "请等待当前操作完成后退出。"; return; }
        dialogOpen = true;
        try
        {
            if (!await ((App)Microsoft.UI.Xaml.Application.Current).ConfirmDraftsAsync()) return;
        }
        finally { dialogOpen = false; }
        ShowPanel();
        var dialog = new ContentDialog
        {
            Title = "退出 Codex Mixin？",
            Content = "退出界面后，本地网关会继续按 CLI 的服务设置运行。",
            PrimaryButtonText = "退出",
            CloseButtonText = "取消",
            XamlRoot = Panel.XamlRoot,
            RequestedTheme = Panel.ActualTheme
        };
        dialogOpen = true;
        try
        {
            if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
            await ((App)Microsoft.UI.Xaml.Application.Current).ExitAllAsync();
        }
        finally { dialogOpen = false; }
    }
    private async Task ChangeAsync(string[] args)
    {
        if (changing) return;
        changing = true;
        GatewaySwitch.IsEnabled = false;
        refreshCancellation?.Cancel();
        statusCancellation?.Cancel();
        await Task.WhenAll(refreshTask, statusRefreshTask);
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
