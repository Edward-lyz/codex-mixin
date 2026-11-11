using System.Diagnostics;
using System.Text.Json;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.ApplicationModel.DataTransfer;
using Windows.Storage.Pickers;
using Windows.System;
using WinRT.Interop;

namespace CodexMixin;

// The XAML compiler requires a public root type; this is not a business API.
public sealed partial class MainWindow : Window
{
    private readonly CancellationTokenSource lifetime = new();
    private readonly DispatcherTimer timer = new() { Interval = TimeSpan.FromSeconds(30) };
    private CliClient? cli;
    private Operations? operations;
    private NativeShell? shell;
    private CancellationTokenSource? activeCancellation;
    private JsonElement metadata;
    private string page;
    private string? queuedPage;
    private string? providerId;
    private JsonElement[] providers = [];
    private readonly Dictionary<string, JsonElement> providerRows = [];
    private static readonly TimeSpan ReadSnapshotAge = TimeSpan.FromSeconds(30);
    private JsonElement[]? providerSnapshot;
    private DateTimeOffset providerSnapshotAt;
    private long providerSnapshotStamp;
    private long providerRevision;
    private JsonElement? benchmarkSnapshot;
    private JsonElement[]? agentSnapshot;
    private DateTimeOffset agentSnapshotAt;
    private long agentSnapshotStamp;
    private long agentRevision;
    private bool connectionSelected;
    private Func<Task>? pendingApply;
    private readonly Dictionary<string, (UIElement View, Func<Task> Save)> connectionDrafts = [];
    private readonly Dictionary<string, (UIElement View, Func<Task> Save)> modelDrafts = [];
    private readonly Dictionary<string, (TextBlock Ttft, TextBlock Tps)> timingCells = [];
    private readonly Dictionary<string, JsonElement> timings = [];
    private bool draftDirty;
    private bool applyingDraft;
    private bool ready;
    private bool selectingProvider;
    private bool interfaceReady;
    private bool selectingNavigation;
    private bool busy;
    private bool loadingPage;
    private bool pageLoaded;
    private CancellationTokenSource? pageRefresh;
    private bool exiting;
    private bool dialogOpen;
    private bool initialized;
    private bool refreshingTimings;

    public MainWindow() : this("providers") { }

    internal MainWindow(string page)
    {
        this.page = page;
        InitializeComponent();
        XamlText.Translate(Content);
        Root.RequestedTheme = ((App)Microsoft.UI.Xaml.Application.Current).Theme;
        ProviderList.ItemTemplate = (DataTemplate)Microsoft.UI.Xaml.Application.Current.Resources["VirtualRowTemplate"];
        ProviderList.ItemContainerStyle = Themed("ModelRowContainer");
        ProviderList.ContainerContentChanging += RenderProviderRow;
        ApplyPageLayout();
        AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "Assets", "app_icon.ico"));
        if (IsConsolePage(page)) AppWindow.MoveAndResize(NativeShell.ConsoleBounds());
        else AppWindow.MoveAndResize(NativeShell.SideWindowBounds());
        if (AppWindowTitleBar.IsCustomizationSupported())
        {
            ExtendsContentIntoTitleBar = true;
            SetTitleBar(TitleBar);
            AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Tall;
            AppWindow.TitleBar.PreferredTheme = Root.ActualTheme == ElementTheme.Dark ? TitleBarTheme.Dark : TitleBarTheme.Light;
            Root.ActualThemeChanged += (_, _) =>
                AppWindow.TitleBar.PreferredTheme = Root.ActualTheme == ElementTheme.Dark ? TitleBarTheme.Dark : TitleBarTheme.Light;
        }
        SystemBackdrop = new MicaBackdrop();
        var handle = WindowNative.GetWindowHandle(this);
        // The console window owns the tray icon; settings is a side window.
        try { if (IsConsolePage(page)) shell = new NativeShell(handle, OnTrayCommand); }
        catch (Exception error) { ShowNotice(Localized.Get("系统托盘不可用"), error.Message, InfoBarSeverity.Warning); }
        AppWindow.Closing += async (_, args) =>
        {
            if (exiting) return;
            if (shell != null) { args.Cancel = true; shell.Hide(); return; }
            if (!draftDirty) return;
            args.Cancel = true;
            if (await ConfirmDiscardAsync(Localized.Get("关闭窗口"))) Shutdown();
        };
        Closed += (_, _) =>
        {
            exiting = true;
            timer.Stop();
            lifetime.Cancel();
            pageRefresh?.Cancel();
            pageRefresh?.Dispose();
            shell?.Dispose();
        };
        Root.Loaded += (_, _) => InitializeView();
        AppWindow.Changed += (_, args) =>
        {
            if (args.DidVisibilityChange || args.DidSizeChange || args.DidPresenterChange)
            {
                InitializeView();
                UpdateRefreshTimer();
                Activity.IsActive = busy && IsViewVisible;
            }
        };
        timer.Tick += async (_, _) =>
        {
            if (!busy && !dialogOpen && page == "overview" && IsViewVisible && cli != null)
                await ExecuteUiAsync(Localized.Get("刷新状态"), LoadPageAsync);
            if (!busy && page == "providers" && !connectionSelected && IsViewVisible && cli != null)
                await RefreshTimingsAsync(lifetime.Token);
        };
        var refresh = new KeyboardAccelerator { Key = VirtualKey.R, Modifiers = VirtualKeyModifiers.Control };
        refresh.Invoked += async (_, args) => { args.Handled = true; await ReloadAsync(); };
        Root.KeyboardAccelerators.Add(refresh);
        // The model table virtualizes its rows, so it gets the visible height instead of the page scroller.
        PageScroll.SizeChanged += (_, _) =>
        {
            foreach (var table in PageBody.Children.OfType<StackPanel>().SelectMany(panel => panel.Children.OfType<ListView>()))
                table.Height = ModelTableHeight();
        };
    }

    private void InitializeView()
    {
        if (exiting || initialized || !Root.IsLoaded || !IsViewVisible) return;
        initialized = true;
        // Loaded can run inside a synchronous native layout callback.
        // Start CLI work after that callback returns to the message loop.
        if (!DispatcherQueue.TryEnqueue(async () =>
        {
            ready = true;
            await ExecuteUiAsync(Localized.Get("连接 CLI"), async token =>
            {
                if (page != "settings") await ConnectCliAsync(token);
                await LoadPageAsync(token);
            }, readOnly: true);
            UpdateRefreshTimer();
        })) throw new InvalidOperationException(Localized.Get("无法调度 Windows 界面初始化。"));
    }

    private bool IsViewVisible => AppWindow.IsVisible && AppWindow.Presenter is not OverlappedPresenter { State: OverlappedPresenterState.Minimized };

    private void UpdateRefreshTimer()
    {
        if (initialized && !exiting && IsViewVisible && (page == "overview" || page == "providers" && !connectionSelected)) timer.Start();
        else timer.Stop();
    }

    internal bool IsChanging => busy && !loadingPage;
    internal bool HasDraftChanges => draftDirty;
    internal bool NotifyUpdate(string version) => shell?.NotifyUpdate(version) == true;
    internal void Shutdown() { exiting = true; Close(); }
    internal void SetTheme(ElementTheme theme) => Root.RequestedTheme = theme;

    /// Pages hosted by the single console window, in tab order.
    internal static bool IsConsolePage(string name) => name is "providers" or "agents" or "context" or "usage" or "routing";

    private void ApplyPageLayout()
    {
        Title = page switch
        {
            "providers" => Localized.Get("模型与服务"),
            "agents" => Localized.Get("Agent 接入管理"),
            "routing" => Localized.Get("模型路由"),
            "usage" => Localized.Get("用量明细"),
            "context" => Localized.Get("上下文"),
            _ => Localized.Get("设置与诊断")
        };
        var console = IsConsolePage(page);
        TitleBar.Title = console ? "Codex Mixin" : $"Codex Mixin · {Title}";
        ConsoleTabs.Visibility = console ? Visibility.Visible : Visibility.Collapsed;
        selectingNavigation = true;
        try { if (console) ConsoleTabs.SelectedItem = ConsoleTabs.Items.First(item => (string)item.Tag == page); }
        finally { selectingNavigation = false; }
        var providers = page == "providers";
        SidebarColumn.Width = new GridLength(providers ? 248 : 0);
        ProviderSidebar.Visibility = providers ? Visibility.Visible : Visibility.Collapsed;
        ProviderTabs.Visibility = ProviderSidebar.Visibility;
        ContentLayer.BorderThickness = providers ? new Thickness(1, 1, 0, 0) : new Thickness(0, 1, 0, 0);
        ContentLayer.CornerRadius = providers ? new CornerRadius(8, 0, 0, 0) : new CornerRadius(0);
        DraftSummary.Visibility = ApplyChanges.Visibility = page is "providers" or "routing" or "context" ? Visibility.Visible : Visibility.Collapsed;
        var header = page == "agents" ? Visibility.Collapsed : Visibility.Visible;
        PageTitle.Visibility = header;
        UpdateRefreshTimer();
    }

    /// Switches the console window to another console page. Unapplied provider
    /// edits are confirmed first because loading a page rebuilds the drafts.
    internal async void ShowPage(string next)
    {
        ShowMain();
        if (busy)
        {
            if (next == page && queuedPage == null) return;
            queuedPage = next;
            // Superseded reads can be cancelled; configuration writes must finish.
            if (loadingPage && interfaceReady) activeCancellation?.Cancel();
            return;
        }
        if (next == page && pageLoaded) return;
        // The draft dictionaries cache every provider's editor; only MarkDirty
        // means the user changed something that is not applied yet.
        if (!await ConfirmDiscardAsync(Localized.Get("切换页面")))
        {
            ApplyPageLayout();
            return;
        }
        connectionDrafts.Clear(); modelDrafts.Clear();
        // Hidden provider views and save closures otherwise retain the full model editor.
        pendingApply = null;
        timingCells.Clear();
        ProviderList.ItemsSource = null; providerRows.Clear(); providers = [];
        page = next;
        ApplyPageLayout();
        if (ready && !interfaceReady && page != "settings") { await ReloadAsync(); return; }
        if (ready) await ExecuteUiAsync(Localized.Get("切换页面"), async token =>
        {
            await LoadPageAsync(token);
        }, readOnly: true);
    }

    private void OnConsoleTab(SelectorBar sender, SelectorBarSelectionChangedEventArgs args)
    {
        if (ready && !selectingNavigation && sender.SelectedItem?.Tag is string next) ShowPage(next);
    }

    internal void ShowMain()
    {
        shell?.Show();
        AppWindow.Show();
        Activate();
        // Native ShowWindow can deliver Loaded before AppWindow reports visibility.
        DispatcherQueue.TryEnqueue(InitializeView);
    }

    private void OnTrayCommand(string command) => DispatcherQueue.TryEnqueue(async () =>
    {
        if (command == "tray-error")
        {
            ShowMain();
            ShowNotice(Localized.Get("系统托盘不可用"), Localized.Get("Explorer 重启后无法恢复托盘，请重新启动应用。"), InfoBarSeverity.Warning);
            return;
        }
        if (command == "exit") { await ExitAsync(lightweight: false); return; }
        if (command == "lightweight") { await ExitAsync(lightweight: true); return; }
        if (command is "start" or "stop")
        {
            if (busy) { ShowMain(); return; }
            await ChangeAsync(command == "start" ? Localized.Get("启动网关") : Localized.Get("停止网关"),
                ["service", command, "--managed", "--json"]);
            return;
        }
        ((App)Microsoft.UI.Xaml.Application.Current).OpenPage(command is "console" or "providers" or "agents" or "usage" or "routing" or "fusion" or "context" or "settings" or "updates" ? command : "overview");
    });

    private async Task ExitAsync(bool lightweight)
    {
        if (((App)Microsoft.UI.Xaml.Application.Current).HasPendingChanges)
        {
            ShowMain();
            ShowNotice(Localized.Get("操作仍在进行"), Localized.Get("请等待当前操作完成后退出。关闭窗口可以收起到托盘。"), InfoBarSeverity.Warning);
            return;
        }
        // A never-shown tray-only window has no XamlRoot for a ContentDialog.
        if (!await ((App)Microsoft.UI.Xaml.Application.Current).ConfirmDraftsAsync()) return;
        var app = (App)Microsoft.UI.Xaml.Application.Current;
        Func<Task> exit = lightweight ? app.EnterLightweightModeAsync : app.QuitCompletelyAsync;
        var confirmed = !initialized || (lightweight
            ? await ConfirmAsync(Localized.Get("进入轻量模式？"), Localized.Get("关闭界面和托盘，只保留后台网关，并开启网关开机启动。需要时重新打开 Codex Mixin 即可恢复界面。"), Localized.Get("进入轻量模式"))
            : await ConfirmAsync(Localized.Get("彻底退出 Mixin？"), Localized.Get("会停止本地网关并关闭界面，通过 Mixin 接入的 Agent 将暂时不可用。"), Localized.Get("彻底退出")));
        if (!confirmed) return;
        try
        {
            exiting = true;
            await exit();
        }
        catch (Exception error)
        {
            exiting = false;
            ShowMain();
            ShowNotice(lightweight ? Localized.Get("进入轻量模式失败") : Localized.Get("退出失败"), error.Message, InfoBarSeverity.Error);
        }
    }

    private async void OnProviderSelected(object sender, SelectionChangedEventArgs args)
    {
        if (!ready || selectingProvider || ProviderList.SelectedItem is not string id) return;
        if (draftDirty)
        {
            selectingProvider = true;
            ProviderList.SelectedItem = providerId;
            try
            {
                if (!await ConfirmDiscardAsync(Localized.Get("切换服务商"))) return;
                ProviderList.SelectedItem = id;
                connectionDrafts.Clear(); modelDrafts.Clear();
            }
            finally { selectingProvider = false; }
        }
        providerId = id;
        RenderSelectedProvider();
    }

    private async void OnAddProvider(object sender, RoutedEventArgs args)
    {
        try
        {
            var hadDraft = draftDirty;
            if (!await ConfirmDiscardAsync(Localized.Get("新增服务商"))) return;
            if (hadDraft) await ReloadAsync();
            await AddProviderAsync();
        }
        catch (Exception error) { ShowNotice(Localized.Get("新增服务商失败"), error.Message, InfoBarSeverity.Error); }
    }
    private async void OnRemoveProvider(object sender, RoutedEventArgs args)
    {
        if (providerId != null && await ConfirmAsync(Localized.Get("删除服务商？"), Localized.Get("将移除当前服务商及其模型配置。"), Localized.Get("删除")))
            await ChangeAsync(Localized.Get("删除服务商"), ["provider", "remove", providerId], apply: true);
    }
    private async void OnProviderTab(SelectorBar sender, SelectorBarSelectionChangedEventArgs args)
    {
        if (!ready || loadingPage || selectingNavigation || sender.SelectedItem == null) return;
        await SelectProviderTabAsync(sender.SelectedItem == ConnectionTab);
    }

    private async Task SelectProviderTabAsync(bool connection)
    {
        if (connection != connectionSelected && await ConfirmDiscardAsync(Localized.Get("切换编辑页")))
        {
            connectionSelected = connection;
            UpdateRefreshTimer();
            RenderSelectedProvider();
        }
        SyncProviderTab();
    }

    private void SyncProviderTab()
    {
        selectingNavigation = true;
        try { ProviderTabs.SelectedItem = connectionSelected ? ConnectionTab : ModelsTab; }
        finally { selectingNavigation = false; }
    }

    private async void OnApplyChanges(object sender, RoutedEventArgs args)
    {
        if (busy || applyingDraft || ((App)Microsoft.UI.Xaml.Application.Current).IsUpdating) return;
        applyingDraft = true;
        try { if (pendingApply != null) await pendingApply(); }
        catch (Exception error) { ShowNotice(Localized.Get("应用更改失败"), error.Message, InfoBarSeverity.Error); }
        finally { applyingDraft = false; }
    }

    private void OnOpenPage(object sender, RoutedEventArgs args) =>
        ((App)Microsoft.UI.Xaml.Application.Current).OpenPage((string)((MenuFlyoutItem)sender).Tag);

    private async void OnRefresh(object sender, RoutedEventArgs args) => await ReloadAsync();
    private void OnCancel(object sender, RoutedEventArgs args) => activeCancellation?.Cancel();

    private async Task ConnectCliAsync(CancellationToken token, bool refresh = false)
    {
        cli ??= ((App)Microsoft.UI.Xaml.Application.Current).Client;
        operations ??= ((App)Microsoft.UI.Xaml.Application.Current).Operations;
        if (interfaceReady && !refresh) return;
        interfaceReady = false;
        var contract = await cli.QueryAsync(["interface", "--json"], token);
        Json.Interface(contract);
        metadata = contract;
        interfaceReady = true;
    }

    private async Task ReloadAsync()
    {
        if (!await ConfirmDiscardAsync(Localized.Get("刷新页面"))) return;
        await ExecuteUiAsync(Localized.Get("刷新"), async token =>
        {
            if (page != "settings") await ConnectCliAsync(token, refresh: true);
            agentSnapshot = null; providerSnapshot = null; benchmarkSnapshot = null;
            await LoadPageAsync(token);
        }, readOnly: true);
    }

    internal async Task<bool> ConfirmDiscardAsync(string action, bool discard = true)
    {
        if (!draftDirty) return true;
        ShowMain();
        if (!await ConfirmAsync(Localized.Get("放弃未应用的更改？"), Localized.Format("{0}会丢弃尚未保存的更改。", action), Localized.Get("放弃更改"))) return false;
        if (discard)
        {
            draftDirty = false;
            connectionDrafts.Clear(); modelDrafts.Clear();
        }
        return true;
    }

    private async Task<bool> ExecuteUiAsync(string label, Func<CancellationToken, Task> work, bool readOnly = false)
    {
        if (busy || exiting || ((App)Microsoft.UI.Xaml.Application.Current).IsUpdating) return false;
        var succeeded = false;
        busy = true;
        loadingPage = readOnly;
        // Disable pointer, keyboard, focus, and automation edits together.
        PageShell.IsEnabled = false;
        Activity.IsActive = IsViewVisible;
        Activity.Visibility = Visibility.Visible;
        StatusText.Text = $"{label}…";
        using var cancellation = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
        activeCancellation = cancellation;
        CancelAction.Visibility = Visibility.Visible;
        try
        {
            await work(cancellation.Token);
            cancellation.Token.ThrowIfCancellationRequested();
            succeeded = true;
            if (!exiting) StatusText.Text = Localized.Format("{0}完成 · {1:HH:mm:ss}", label, DateTime.Now);
        }
        catch (Exception error) when (readOnly && queuedPage != null && cancellation.IsCancellationRequested
            && error is CliFailure or OperationCanceledException)
        {
            // A newer navigation request will replace this cancelled snapshot.
        }
        catch (Exception error)
        {
            if (exiting) return false;
            var title = error is CliFailure { Committed: true } ? Localized.Get("配置已保存，后续操作失败") : Localized.Format("{0}失败", label);
            StatusText.Text = title;
            ShowNotice(title, error.Message, InfoBarSeverity.Error);
            if (error is CliFailure { Committed: true })
            {
                PageBody.Children.Clear();
                try { await LoadPageAsync(cancellation.Token); }
                catch (Exception refreshError)
                {
                    PageBody.Children.Clear();
                    Notice.Message += Localized.Format("\n状态刷新失败：{0}", refreshError.Message);
                }
            }
            // Never leave a failed status query showing a previously healthy gateway.
            if (page == "overview")
            {
                PageBody.Children.Clear();
                PageBody.Children.Add(Card(Localized.Get("状态未知"), Localized.Get("无法读取当前状态，请刷新重试。"), Text(error.Message)));
            }
        }
        finally
        {
            activeCancellation = null;
            CancelAction.Visibility = Visibility.Collapsed;
            busy = false;
            loadingPage = false;
            PageShell.IsEnabled = true;
            Activity.IsActive = false;
            Activity.Visibility = Visibility.Collapsed;
        }
        if (queuedPage is { } next && !exiting)
        {
            queuedPage = null;
            ShowPage(next);
        }
        return succeeded;
    }

    private async Task<bool> ChangeAsync(string label, string[] args, bool apply = false,
        IReadOnlyDictionary<string, string>? secrets = null)
    {
        if (busy || exiting || !applyingDraft && !await ConfirmDiscardAsync(label)) return false;
        return await ExecuteUiAsync(label, async token =>
    {
        await ConnectCliAsync(token);
        var service = operations ?? throw new InvalidOperationException(Localized.Get("CLI 尚未连接，请先刷新。"));
        var progress = new Progress<string>(message => StatusText.Text = message);
        var output = await service.ChangeAsync(args, token, secrets, progress, apply);
        draftDirty = false;
        if (apply) ShowSummary(Localized.Get("配置已同步"));
        ShowNotice(label, string.IsNullOrWhiteSpace(output) ? Localized.Get("操作完成。") : output.Trim(), InfoBarSeverity.Success);
        // A tray-only startup has not loaded the console; do not build a hidden model page.
        if (!initialized) return;
        try { await LoadPageAsync(token); if (apply) ShowSummary(Localized.Get("配置已同步")); }
        catch (Exception error)
        {
            ShowSummary(Localized.Get("配置已保存，状态刷新失败"));
            ShowNotice(Localized.Format("{0}完成，状态刷新失败", label), error.Message, InfoBarSeverity.Warning);
            PageBody.Children.Clear();
            PageBody.Children.Add(Card(Localized.Get("状态刷新失败"), Localized.Get("操作已执行，请刷新确认最新状态。"), Text(error.Message)));
        }
    });
    }

    private async Task LoadPageAsync(CancellationToken token)
    {
        pageLoaded = false;
        pageRefresh?.Cancel();
        pageRefresh?.Dispose();
        pageRefresh = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
        PageBody.Children.Clear();
        draftDirty = false;
        pendingApply = null;
        ApplyChanges.IsEnabled = false;
        ApplyChanges.Content = Localized.Get("应用更改");
        ProviderHeaderTile.Visibility = StatusBadge.Visibility = OfficialBadge.Visibility = Visibility.Collapsed;
        ShowSummary(Localized.Get("未修改"));
        ToolTipService.SetToolTip(PageTitle, null);
        PageTitle.Text = page switch
        {
            "providers" => Localized.Get("供应商与模型"),
            "agents" => Localized.Get("Agent 接入"),
            "usage" => Localized.Get("用量明细"),
            "context" => Localized.Get("上下文"),
            "routing" => Localized.Get("模型路由"),
            "settings" => Localized.Get("设置"),
            _ => Localized.Get("概览")
        };
        switch (page)
        {
            case "providers": await ProvidersAsync(token); break;
            case "agents": await AgentsAsync(token); break;
            case "usage": await UsageAsync(token); break;
            case "context": await ContextAsync(token); break;
            case "routing": await RoutingAsync(token); break;
            case "settings": await SettingsAsync(token); break;
            default: await OverviewAsync(token); break;
        }
        token.ThrowIfCancellationRequested();
        pageLoaded = true;
    }

    private CliClient Client => cli ?? throw new InvalidOperationException(Localized.Get("CLI 尚未连接，请刷新。"));

    private async Task OverviewAsync(CancellationToken token)
    {
        var status = await Client.QueryAsync(["service", "status", "--json"], token);
        var state = Json.Gateway(status);
        var endpoint = Json.OptionalText(status, "endpoint");
        var buttons = Row();
        buttons.Children.Add(Button(Localized.Get("启动网关"), () => ChangeAsync(Localized.Get("启动网关"), ["service", "start", "--managed", "--json"]), true));
        buttons.Children.Add(Button(Localized.Get("停止网关"), () => ChangeAsync(Localized.Get("停止网关"), ["service", "stop", "--managed", "--json"])));
        buttons.Children.Add(Button(Localized.Get("重启"), () => ChangeAsync(Localized.Get("重启网关"), ["service", "restart", "--managed", "--json"])));
        if (endpoint != null) buttons.Children.Add(Button(Localized.Get("复制地址"), () =>
        {
            var content = new DataPackage(); content.SetText(endpoint); Clipboard.SetContent(content);
            StatusText.Text = Localized.Get("接口地址已复制");
            return Task.CompletedTask;
        }));
        PageBody.Children.Add(Card(Localized.Format("本地网关 · {0}", state), endpoint ?? Localized.Get("尚无运行中的接口地址"), buttons));
        if (Json.OptionalText(status, "error") is { } statusError)
            PageBody.Children.Add(Card(Localized.Get("服务诊断"), statusError));
        var providers = Json.Flag(status, "configured")
            ? Json.Rows(Json.Field(await Client.QueryAsync(["provider", "list", "--json"], token), "providers"))
            : [];
        if (providers.Length == 0) PageBody.Children.Add(Card(Localized.Get("开始连接模型服务"), Localized.Get("先添加一个供应商，再选择模型并接入 Agent。"),
            Button(Localized.Get("添加供应商"), AddProviderAsync, true)));
        foreach (var provider in providers)
        {
            var selected = Json.Strings(Json.Field(provider, "selected_models"));
            PageBody.Children.Add(Card(Json.Text(provider, "display_name"),
                Localized.Format("{0} · {1} 个模型", Readiness(provider), selected.Length), Text(string.Join("  ·  ", selected))));
        }
    }

    private static string Readiness(JsonElement provider) => !Json.Flag(provider, "enabled") ? Localized.Get("已停用") :
        Json.Text(provider, "readiness") switch { "healthy" => Localized.Get("正常"), "degraded" => Localized.Get("降级"), var value => value };

    private static string ReadinessDot(JsonElement provider) => !Json.Flag(provider, "enabled") ? "DotOff" :
        Json.Text(provider, "readiness") == "healthy" ? "DotOn" : "DotWarn";

    // Styles carry ThemeResource brushes, so code-built elements follow the window theme.
    private static Style Themed(string key) => (Style)Microsoft.UI.Xaml.Application.Current.Resources[key];

    private static StackPanel IconLabel(string glyph, string text)
    {
        var label = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        label.Children.Add(new FontIcon { Glyph = glyph, FontSize = 14 });
        label.Children.Add(new TextBlock { Text = text });
        return label;
    }

    private MenuFlyoutItem MenuItem(string label, string glyph, Func<Task> work)
    {
        var item = new MenuFlyoutItem { Text = label, Icon = new FontIcon { Glyph = glyph } };
        item.Click += async (_, _) =>
        {
            try { await work(); }
            catch (Exception error) { ShowNotice(Localized.Format("{0}失败", label), error.Message, InfoBarSeverity.Error); }
        };
        return item;
    }

    private async Task ProvidersAsync(CancellationToken token)
    {
        var revision = ((App)Microsoft.UI.Xaml.Application.Current).Operations.Revision;
        if (providerSnapshot == null || providerRevision != revision || Stopwatch.GetElapsedTime(providerSnapshotStamp) >= ReadSnapshotAge)
        {
            providerSnapshot = Json.Rows(Json.Field(await Client.QueryAsync(["provider", "list", "--json"], token), "providers"));
            providerSnapshotAt = DateTimeOffset.UtcNow;
            providerSnapshotStamp = Stopwatch.GetTimestamp();
            providerRevision = revision;
        }
        providers = providerSnapshot;
        selectingProvider = true;
        try
        {
            // A reload shows the saved state; drafts only survive provider and tab switches.
            modelDrafts.Clear(); connectionDrafts.Clear(); timingCells.Clear();
            providerRows.Clear();
            foreach (var provider in providers)
            {
                var id = Json.Text(provider, "id");
                foreach (var field in new[] { "display_name", "readiness", "icon" }) Json.Text(provider, field);
                Json.Flag(provider, "enabled");
                foreach (var field in new[] { "selected_models", "cached_models" })
                    if (Json.Field(provider, field).ValueKind != JsonValueKind.Array) throw new CliFailure(Localized.Format("供应商 {0} 的 {1} 必须是 array。", id, field));
                providerRows.Add(id, provider);
            }
            ProviderList.ItemsSource = providers.Select(provider => Json.Text(provider, "id")).ToArray();
        }
        finally { selectingProvider = false; }
        if (providers.Length == 0)
        {
            pendingApply = null; ApplyChanges.IsEnabled = false;
            PageTitle.Text = Localized.Get("还没有服务商");
            PageBody.Children.Add(Button(Localized.Get("新增服务商"), AddProviderAsync, true));
            return;
        }
        // SelectionChanged renders once after the list has its final selection.
        ProviderList.SelectedIndex = Math.Max(0, Array.FindIndex(providers, row => Json.Text(row, "id") == providerId));
        // Render the local snapshot first; benchmark IPC is not on the navigation path.
        _ = RefreshTimingsAsync(pageRefresh!.Token);
    }

    private void RenderProviderRow(ListViewBase sender, ContainerContentChangingEventArgs args)
    {
        if (args.ItemContainer.ContentTemplateRoot is not ContentControl content) return;
        content.Content = null;
        if (args.InRecycleQueue || args.Item is not string id || !providerRows.TryGetValue(id, out var provider)) return;
        var row = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        row.Children.Add(new TextBlock { Text = Json.Text(provider, "display_name"), TextTrimming = TextTrimming.CharacterEllipsis });
        var state = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        state.Children.Add(new Microsoft.UI.Xaml.Shapes.Ellipse { Style = Themed(ReadinessDot(provider)), VerticalAlignment = VerticalAlignment.Center });
        state.Children.Add(new TextBlock { Text = Localized.Format("{0} · {1}/{2} 个模型", Readiness(provider), Json.Field(provider, "selected_models").GetArrayLength(), Json.Field(provider, "cached_models").GetArrayLength()), Style = Themed("Caption") });
        row.Children.Add(state);
        var tile = new Grid { ColumnSpacing = 12 };
        tile.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        tile.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        tile.Children.Add(new Border { Style = Themed("IconTile"), Width = 36, Height = 36, Child = Marks.Provider(provider, 22) });
        Grid.SetColumn(row, 1); tile.Children.Add(row);
        content.Content = new Border { Child = tile, Padding = new Thickness(8, 6, 8, 6), Margin = new Thickness(0, 1, 0, 1) };
        args.Handled = true;
    }

    private async void RenderSelectedProvider()
    {
        if (providerId == null) return;
        var provider = providerRows.GetValueOrDefault(providerId);
        if (provider.ValueKind != JsonValueKind.Object) return;
        PageBody.Children.Clear();
        SyncProviderTab();
        PageTitle.Text = Json.Text(provider, "display_name");
        ProviderHeaderIcon.Source = Marks.Provider(provider).Source;
        ProviderHeaderTile.Visibility = Visibility.Visible;
        OfficialBadge.Visibility = Json.Text(provider, "kind") == "official" ? Visibility.Visible : Visibility.Collapsed;
        var healthy = Json.Flag(provider, "enabled") && Json.Text(provider, "readiness") == "healthy";
        StatusBadge.Style = Themed(healthy ? "SuccessBadge" : "CautionBadge");
        StatusBadgeText.Text = healthy ? Localized.Get("配置就绪") : Readiness(provider);
        StatusBadge.Visibility = Visibility.Visible;
        var refreshed = provider.TryGetProperty("models_refreshed_at_ms", out var stamp) && stamp.ValueKind == JsonValueKind.Number
            ? Localized.Format(" · 上次刷新 {0:MM/dd HH:mm}", DateTimeOffset.FromUnixTimeMilliseconds(stamp.GetInt64()).ToLocalTime()) : "";
        ToolTipService.SetToolTip(PageTitle, Localized.Format("{0} · 已加入 {1} / 可选 {2}{3} · 配置 {4:HH:mm:ss}", providerId, Json.Field(provider, "selected_models").GetArrayLength(), Json.Field(provider, "cached_models").GetArrayLength(), refreshed, providerSnapshotAt.ToLocalTime()));
        ApplyChanges.IsEnabled = draftDirty;
        try
        {
            if (connectionSelected)
            {
                if (Json.Text(provider, "kind") == "official")
                {
                    PageBody.Children.Add(Card(Localized.Get("连接配置"), Localized.Get("官方服务商由 Codex OAuth 管理，连接参数只读。")));
                    pendingApply = null; ApplyChanges.IsEnabled = false; return;
                }
                if (connectionDrafts.TryGetValue(providerId, out var draft))
                {
                    PageBody.Children.Add(draft.View); pendingApply = draft.Save;
                }
                else await ProviderDialogAsync(provider, inline: true);
            }
            else if (modelDrafts.TryGetValue(providerId, out var modelDraft))
            {
                PageBody.Children.Add(modelDraft.View); pendingApply = modelDraft.Save;
            }
            else RenderModels(provider);
            if (!connectionSelected && benchmarkSnapshot is { } snapshot) ApplyTimings(snapshot);
        }
        catch (Exception error) { ShowNotice(Localized.Get("读取服务商失败"), error.Message, InfoBarSeverity.Error); }
    }

    private void RenderModels(JsonElement provider)
    {
        var modelBody = new StackPanel { Spacing = 8 };
        var id = Json.Text(provider, "id");
        var models = Json.Rows(Json.Field(provider, "cached_models"));
        var modelsById = models.Select((model, index) => (Model: model, Index: index)).ToDictionary(row => Json.Text(row.Model, "id"));
        var modelIds = modelsById.Keys.ToArray();
        var selected = Json.Strings(Json.Field(provider, "selected_models")).ToList();
        var modelOrder = Json.OptionalText(provider, "model_order") ?? "family";
        var savedContexts = Json.Field(provider, "model_context_overrides");
        var contexts = savedContexts.EnumerateObject().ToDictionary(entry => entry.Name, entry => entry.Value.GetDouble());

        var toolbar = new Grid { ColumnSpacing = 8 };
        foreach (var width in new[] { new GridLength(1, GridUnitType.Star), GridLength.Auto, GridLength.Auto, GridLength.Auto, GridLength.Auto })
            toolbar.ColumnDefinitions.Add(new ColumnDefinition { Width = width });
        var search = new AutoSuggestBox { PlaceholderText = Localized.Get("搜索当前服务商的模型"), QueryIcon = new SymbolIcon(Symbol.Find), MaxWidth = 360, HorizontalAlignment = HorizontalAlignment.Stretch };
        AutomationProperties.SetName(search, Localized.Get("搜索当前服务商的模型"));
        var filter = new ComboBox { MinWidth = 108, VerticalAlignment = VerticalAlignment.Center };
        foreach (var option in new[] { Localized.Get("全部"), Localized.Get("已加入"), Localized.Get("未加入") }) filter.Items.Add(option);
        filter.SelectedIndex = 0;
        AutomationProperties.SetName(filter, Localized.Get("模型筛选"));
        var actions = new MenuFlyout { Placement = Microsoft.UI.Xaml.Controls.Primitives.FlyoutPlacementMode.BottomEdgeAlignedLeft };
        actions.Items.Add(MenuItem(Localized.Get("刷新模型列表"), "\uE72C", () => ChangeAsync(Localized.Get("刷新模型"), ["provider", "discover", id], apply: true)));
        actions.Items.Add(MenuItem(Localized.Get("重新探测已加入模型的能力"), "\uE9D9", () => ChangeAsync(Localized.Get("探测模型能力"), ["provider", "probe", id], apply: true)));
        var more = new DropDownButton { Content = new FontIcon { Glyph = "\uE712", FontSize = 14 }, Flyout = actions, VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetName(more, Localized.Get("更多模型操作"));
        var test = Button(Localized.Format("测试已加入的 {0} 个模型", selected.Count), () => ChangeAsync(Localized.Get("启动模型测速"), ["benchmark", "start", "--provider", id, "--timeout-seconds", "60"]), true);
        test.Content = IconLabel("\uE916", Localized.Format("测试已加入的 {0} 个模型", selected.Count));
        AutomationProperties.SetName(test, Localized.Format("测试已加入的 {0} 个模型", selected.Count));
        var order = new ComboBox { MinWidth = 128, VerticalAlignment = VerticalAlignment.Center };
        order.Items.Add(Localized.Get("家族与版本")); order.Items.Add(Localized.Get("手动排序"));
        order.SelectedIndex = modelOrder == "manual" ? 1 : 0;
        AutomationProperties.SetName(order, Localized.Get("模型排序"));
        ToolTipService.SetToolTip(order, Localized.Get("拖动已加入模型，或使用行菜单调整顺序。更改在应用后生效。"));
        var tools = new FrameworkElement[] { search, filter, order, more, test };
        for (var index = 0; index < tools.Length; index++) { Grid.SetColumn(tools[index], index); toolbar.Children.Add(tools[index]); }
        modelBody.Children.Add(toolbar);

        // Shared by the header and every virtualized row so the columns stay aligned.
        Grid Columns()
        {
            var grid = new Grid { ColumnSpacing = 12 };
            foreach (var width in new[] { 64d, 64d, -1d, 76d, 84d, 80d, 48d, 120d })
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = width < 0 ? new GridLength(1, GridUnitType.Star) : new GridLength(width) });
            return grid;
        }
        var header = Columns();
        var labels = new[] { Localized.Get("加入"), Localized.Get("顺序"), Localized.Get("模型"), Localized.Get("首 Token"), Localized.Get("生成速度"), Localized.Get("上下文"), Localized.Get("倍率"), Localized.Get("能力") };
        for (var index = 0; index < labels.Length; index++)
        {
            var label = new TextBlock { Text = labels[index], Style = Themed("ColumnHeader"), HorizontalAlignment = index is >= 3 and <= 6 ? HorizontalAlignment.Right : HorizontalAlignment.Left };
            Grid.SetColumn(label, index); header.Children.Add(label);
        }
        modelBody.Children.Add(new Border { Style = Themed("RowDivider"), Padding = new Thickness(12, 8, 12, 8), Child = header });

        var list = new ListView { SelectionMode = ListViewSelectionMode.Single, Height = ModelTableHeight(), Padding = new Thickness(0) };
        list.ItemTemplate = (DataTemplate)Microsoft.UI.Xaml.Application.Current.Resources["VirtualRowTemplate"];
        list.ItemContainerStyle = Themed("ModelRowContainer");
        AutomationProperties.SetAutomationId(list, "model-table");
        list.ContainerContentChanging += (_, args) =>
        {
            if (args.ItemContainer.Tag is ValueTuple<string, TextBlock> previous
                && timingCells.TryGetValue(previous.Item1, out var retained)
                && ReferenceEquals(retained.Ttft, previous.Item2)) timingCells.Remove(previous.Item1);
            args.ItemContainer.Tag = null;
            if (args.InRecycleQueue)
            {
                if (args.ItemContainer.ContentTemplateRoot is ContentControl recycled) recycled.Content = null;
                return;
            }
            if (args.Item is not string modelId) return;
            var (model, modelIndex) = modelsById[modelId];
            var row = Columns();
            var check = new CheckBox { IsChecked = selected.Contains(modelId), MinWidth = 0, MinHeight = 0, Padding = new Thickness(0), VerticalAlignment = VerticalAlignment.Center };
            AutomationProperties.SetName(check, Localized.Format("{0} 加入 Codex", modelId));
            check.Checked += (_, _) => { if (!selected.Contains(modelId)) selected.Add(modelId); MarkDirty(); Filter(); };
            check.Unchecked += (_, _) => { selected.Remove(modelId); MarkDirty(); Filter(); };
            var rank = new TextBlock { Text = selected.Contains(modelId) ? $"{selected.IndexOf(modelId) + 1}  ≡" : "", Style = Themed("Caption"), VerticalAlignment = VerticalAlignment.Center };
            var moveMenu = new MenuFlyout();
            async Task Move(int delta)
            {
                var current = selected.IndexOf(modelId);
                var target = current + delta;
                if (current < 0 || target < 0 || target >= selected.Count) return;
                selected.RemoveAt(current); selected.Insert(target, modelId);
                modelOrder = "manual"; order.SelectedIndex = 1; MarkDirty(); Filter();
                await Task.CompletedTask;
            }
            var up = MenuItem(Localized.Get("上移"), "\uE74A", () => Move(-1));
            var down = MenuItem(Localized.Get("下移"), "\uE74B", () => Move(1));
            moveMenu.Items.Add(up); moveMenu.Items.Add(down);
            moveMenu.Opening += (_, _) =>
            {
                up.IsEnabled = selected.IndexOf(modelId) > 0;
                down.IsEnabled = selected.Contains(modelId) && selected.IndexOf(modelId) < selected.Count - 1;
            };
            args.ItemContainer.ContextFlyout = moveMenu;
            ToolTipService.SetToolTip(rank, Localized.Get("拖动排序；右键菜单可上移、下移。"));
            var title = new Grid { ColumnSpacing = 0, VerticalAlignment = VerticalAlignment.Center };
            title.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            title.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var name = new TextBlock { Text = modelId, Style = Themed("MonoPrimary"), MaxWidth = 280 };
            AutomationProperties.SetName(name, modelId);
            title.Children.Add(name);
            ToolTipService.SetToolTip(title, string.Join("\n", new[] { modelId, Json.OptionalText(model, "display_name"), Json.OptionalText(model, "description") }.Where(value => !string.IsNullOrEmpty(value)).Distinct()));
            var originalContext = model.TryGetProperty("context_window", out var original) && original.ValueKind == JsonValueKind.Number ? original.GetDouble() : double.NaN;
            var contextText = new TextBlock { Style = Themed("MonoPrimary") };
            var overridden = new FontIcon { Glyph = "\uE7A7", FontSize = 10, Visibility = Visibility.Collapsed, VerticalAlignment = VerticalAlignment.Center };
            ToolTipService.SetToolTip(overridden, Localized.Get("已覆盖上游上下文"));
            void ShowContext()
            {
                var value = contexts.GetValueOrDefault(modelId, double.NaN);
                overridden.Visibility = double.IsNaN(value) ? Visibility.Collapsed : Visibility.Visible;
                if (double.IsNaN(value)) value = originalContext;
                contextText.Text = double.IsNaN(value) ? Localized.Get("未知") : value >= 1000 ? $"{value / 1000:N0} K" : $"{value:0}";
                ToolTipService.SetToolTip(contextText, double.IsNaN(value) ? Localized.Get("服务商未提供上下文窗口；可刷新模型目录或手动设置。") : $"{value:N0} tokens");
            }
            ShowContext();
            var contextLabel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, HorizontalAlignment = HorizontalAlignment.Right };
            contextLabel.Children.Add(contextText); contextLabel.Children.Add(overridden);
            var context = Button(Localized.Get("编辑上下文"), async () =>
            {
                var number = new NumberBox { Header = Localized.Get("上下文 Token 数"), Minimum = 1, Value = contexts.GetValueOrDefault(modelId, originalContext), SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
                var dialog = Dialog(Localized.Get("模型上下文"), number); dialog.PrimaryButtonText = Localized.Get("应用到草稿"); dialog.SecondaryButtonText = Localized.Get("恢复默认");
                var choice = await ShowDialogAsync(dialog);
                if (choice == ContentDialogResult.None) return;
                if (choice == ContentDialogResult.Primary && (!double.IsFinite(number.Value) || number.Value <= 0 || number.Value != Math.Truncate(number.Value)))
                    throw new InvalidOperationException(Localized.Get("上下文 Token 数必须是正整数。"));
                contexts[modelId] = choice == ContentDialogResult.Secondary ? double.NaN : number.Value;
                ShowContext(); MarkDirty();
            });
            context.Content = contextLabel; context.Style = Themed("FlatButton");
            context.Padding = new Thickness(4, 2, 4, 2); context.HorizontalAlignment = HorizontalAlignment.Right; context.MinHeight = 0;
            AutomationProperties.SetName(context, Localized.Format("{0} 上下文", modelId));
            var capabilities = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, VerticalAlignment = VerticalAlignment.Center };
            foreach (var (field, label, glyph) in new[] { ("supports_image", Localized.Get("图片输入"), "\uE8B9"), ("supports_thinking", "Thinking", "\uE82F"), ("supports_function_tools", "Function Tools", "\uE90F"), ("supports_tool_search", "Tool Search", "\uE721"), ("supports_web_search", "Web Search", "\uE774") })
            {
                var known = model.TryGetProperty(field, out var support) && support.ValueKind is JsonValueKind.True or JsonValueKind.False;
                var supported = known && support.ValueKind == JsonValueKind.True;
                var icon = new FontIcon { Glyph = glyph, Style = Themed(supported ? "GlyphOn" : "GlyphOff") };
                var summary = $"{label} · {(known ? supported ? Localized.Get("支持") : Localized.Get("不支持") : Localized.Get("尚未探测"))}";
                ToolTipService.SetToolTip(icon, summary); AutomationProperties.SetName(icon, summary);
                capabilities.Children.Add(icon);
            }
            var ttft = new TextBlock { Text = "-", Style = Themed("Mono"), HorizontalAlignment = HorizontalAlignment.Right };
            var tps = new TextBlock { Text = "-", Style = Themed("Mono"), HorizontalAlignment = HorizontalAlignment.Right };
            timingCells[$"{id}/{modelId}"] = (ttft, tps);
            args.ItemContainer.Tag = ($"{id}/{modelId}", ttft);
            if (timings.TryGetValue($"{id}/{modelId}", out var timing)) ShowTiming(timing, ttft, tps);
            var ratio = new TextBlock { Text = Json.OptionalText(model, "ratio") ?? "-", Style = Themed("MonoPrimary"), HorizontalAlignment = HorizontalAlignment.Right };
            var cells = new FrameworkElement[] { check, rank, title, ttft, tps, context, ratio, capabilities };
            for (var index = 0; index < cells.Length; index++) { Grid.SetColumn(cells[index], index); row.Children.Add(cells[index]); }
            args.ItemContainer.HorizontalContentAlignment = HorizontalAlignment.Stretch;
            ((ContentControl)args.ItemContainer.ContentTemplateRoot).Content = new Border
            {
                Child = row,
                Style = Themed(modelIndex % 2 == 0 ? "RowStripe" : "RowPlain"),
                Padding = new Thickness(12, 0, 12, 0),
                Height = 34
            };
            args.Handled = true;
        };
        // WinRT collections can expose strings directly; JsonElement is a .NET struct.
        list.CanDragItems = true;
        list.AllowDrop = true;
        string? draggedModel = null;
        list.DragItemsStarting += (_, args) =>
        {
            draggedModel = args.Items.FirstOrDefault() as string;
            if (draggedModel is null || !selected.Contains(draggedModel)) { args.Cancel = true; draggedModel = null; }
        };
        list.DragOver += (_, args) =>
        {
            if (draggedModel is not null) args.AcceptedOperation = DataPackageOperation.Move;
        };
        list.Drop += (_, args) =>
        {
            if (draggedModel is null) return;
            DependencyObject? target = args.OriginalSource as DependencyObject;
            while (target is not null && target is not ListViewItem) target = VisualTreeHelper.GetParent(target);
            if (target is ListViewItem container && container.Content is string targetId && selected.Contains(targetId))
            {
                var position = selected.IndexOf(targetId);
                selected.Remove(draggedModel); selected.Insert(position, draggedModel);
                modelOrder = "manual"; order.SelectedIndex = 1; MarkDirty(); Filter();
            }
            draggedModel = null;
        };
        list.DragItemsCompleted += (_, _) => draggedModel = null;
        list.ItemsSource = modelIds;
        void Filter()
        {
            var text = search.Text;
            var mode = filter.SelectedIndex;
            list.ItemsSource = modelIds.Where(modelId => modelId.Contains(text, StringComparison.OrdinalIgnoreCase) &&
                (mode == 0 || selected.Contains(modelId) == (mode == 1)))
                .OrderBy(modelId => modelOrder == "manual" && selected.Contains(modelId) ? selected.IndexOf(modelId) : int.MaxValue)
                .ToArray();
        }
        search.TextChanged += (_, _) => Filter(); filter.SelectionChanged += (_, _) => Filter();
        order.SelectionChanged += (_, _) => { modelOrder = order.SelectedIndex == 1 ? "manual" : "family"; MarkDirty(); Filter(); };
        Filter();
        modelBody.Children.Add(list);
        ShowSummary(Localized.Get("正在读取测速结果…"));
        pendingApply = async () =>
        {
            var arguments = new List<string> { "provider", "select", id, "--model-order", modelOrder };
            foreach (var modelId in selected) arguments.AddRange(["--model", modelId]);
            foreach (var (modelId, value) in contexts)
            {
                if (double.IsNaN(value)) { if (savedContexts.TryGetProperty(modelId, out _)) arguments.AddRange(["--clear-model-context", modelId]); continue; }
                if (!double.IsFinite(value) || value <= 0 || value != Math.Truncate(value)) throw new InvalidOperationException(Localized.Get("上下文 Token 数必须是正整数。"));
                arguments.AddRange(["--model-context", $"{modelId}={value:0}"]);
            }
            await ChangeAsync(Localized.Get("应用模型更改"), arguments.ToArray(), apply: true);
        };
        modelDrafts[id] = (modelBody, pendingApply);
        PageBody.Children.Add(modelBody);
    }

    private double ModelTableHeight() => Math.Max(240, PageScroll.ActualHeight - 112);

    internal void SetUpdateInputBlocked(bool blocked) => OperationScope.IsEnabled = !blocked;

    private void MarkDirty()
    {
        if (loadingPage || ((App)Microsoft.UI.Xaml.Application.Current).IsUpdating) return;
        draftDirty = true;
        ShowSummary(Localized.Get("还有未应用的更改"));
        ApplyChanges.IsEnabled = true;
    }

    private void TrackDraft(FrameworkElement control)
    {
        switch (control)
        {
            case TextBox box: box.TextChanged += (_, _) => MarkDirty(); break;
            case PasswordBox box: box.PasswordChanged += (_, _) => MarkDirty(); break;
            case ComboBox picker: picker.SelectionChanged += (_, _) => MarkDirty(); break;
            case NumberBox number:
                number.ValueChanged += (_, _) => MarkDirty();
                var textTracked = false;
                number.Loaded += (_, _) =>
                {
                    if (textTracked) return;
                    var descendants = new Queue<DependencyObject>();
                    descendants.Enqueue(number);
                    while (descendants.TryDequeue(out var element))
                    {
                        if (element is TextBox input)
                        {
                            input.TextChanged += (_, _) => MarkDirty();
                            textTracked = true;
                            break;
                        }
                        for (var index = 0; index < VisualTreeHelper.GetChildrenCount(element); index++)
                            descendants.Enqueue(VisualTreeHelper.GetChild(element, index));
                    }
                };
                break;
            case ToggleSwitch toggle: toggle.Toggled += (_, _) => MarkDirty(); break;
            case CheckBox check:
                check.Checked += (_, _) => MarkDirty(); check.Unchecked += (_, _) => MarkDirty(); break;
            case TimePicker time: time.TimeChanged += (_, _) => MarkDirty(); break;
        }
    }

    private void ShowSummary(string text)
    {
        if (DraftSummary.Text != text)
        {
            DraftSummary.Text = text;
            FrameworkElementAutomationPeer.FromElement(DraftSummary)?.RaiseAutomationEvent(AutomationEvents.LiveRegionChanged);
        }
        ToolTipService.SetToolTip(ApplyChanges, text);
        AutomationProperties.SetHelpText(ApplyChanges, text);
    }

    private static string Milliseconds(double value) => value < 1000 ? $"{value:0} ms" : $"{value / 1000:0.##} s";

    private static void ShowTiming(JsonElement row, TextBlock ttft, TextBlock tps)
    {
        var status = Json.Text(row, "status");
        var latency = row.TryGetProperty("ttft_ms", out var ms) && ms.ValueKind == JsonValueKind.Number ? ms.GetDouble() : (double?)null;
        (ttft.Text, ttft.Style) = status switch
        {
            "timed_out" => (Localized.Get("超时"), Themed("MonoCaution")),
            "failed" => (Localized.Get("失败"), Themed("MonoCritical")),
            _ when latency is { } value => (Milliseconds(value), Themed(value < 1000 ? "MonoSuccess" : value < 3000 ? "MonoCaution" : "MonoCritical")),
            _ => ("-", Themed("Mono"))
        };
        var speed = row.TryGetProperty("tps", out var rate) && rate.ValueKind == JsonValueKind.Number ? rate.GetDouble() : (double?)null;
        (tps.Text, tps.Style) = speed is { } tokens ? ($"{tokens:0.0} tok/s", Themed("MonoPrimary")) : ("-", Themed("Mono"));
        ToolTipService.SetToolTip(ttft, Json.OptionalText(row, "error"));
    }

    private async Task RefreshTimingsAsync(CancellationToken token)
    {
        if (refreshingTimings) return;
        refreshingTimings = true;
        try
        {
            var selectedProvider = providerId;
            var response = await Client.QueryAsync(["benchmark", "status"], token);
            token.ThrowIfCancellationRequested();
            if (page != "providers" || providerId != selectedProvider) return;
            var snapshot = Json.Field(response, "snapshot");
            benchmarkSnapshot = snapshot;
            ApplyTimings(snapshot);
        }
        catch (Exception error)
        {
            if (!token.IsCancellationRequested && !exiting && page == "providers" && !connectionSelected && !draftDirty)
                ShowSummary(Localized.Format("无法刷新测速结果：{0}", error.Message));
        }
        finally { refreshingTimings = false; }
    }

    private void ApplyTimings(JsonElement snapshot)
    {
        if (page != "providers" || connectionSelected) return;
        timings.Clear();
        foreach (var (ttft, tps) in timingCells.Values)
        {
            ttft.Text = tps.Text = "-";
            ttft.Style = tps.Style = Themed("Mono");
            ToolTipService.SetToolTip(ttft, null);
        }
        if (snapshot.ValueKind == JsonValueKind.Null)
        {
            if (!draftDirty) ShowSummary(Localized.Get("当前服务商暂无测速结果"));
            return;
        }
        var results = Json.Rows(Json.Field(snapshot, "results")).Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
        foreach (var row in results)
        {
            var key = $"{Json.Text(row, "provider_id")}/{Json.Text(row, "upstream_model")}";
            timings[key] = row;
            if (timingCells.TryGetValue(key, out var cells)) ShowTiming(row, cells.Ttft, cells.Tps);
        }
        if (draftDirty) return;
        if (results.Length == 0) { ShowSummary(Localized.Get("当前服务商暂无测速结果")); return; }
        var started = DateTimeOffset.FromUnixTimeMilliseconds(Json.Field(snapshot, "started_at").GetInt64()).ToLocalTime();
        var prefix = Localized.Format("{0:yyyy年M月d日 HH:mm} · 超时 {1} 秒 · ", started, Json.Field(snapshot, "timeout_seconds"));
        var completed = results.Count(row => Json.Text(row, "status") == "completed");
        var timedOut = results.Count(row => Json.Text(row, "status") == "timed_out");
        var failed = results.Length - completed - timedOut;
        switch (Json.Text(snapshot, "status"))
        {
            case "running": ShowSummary(Localized.Format("{0}正在测速 {1}/{2}", prefix, results.Length, Json.Field(snapshot, "total_models"))); break;
            case "failed": ShowSummary(Localized.Format("{0}测速任务失败：{1}", prefix, Json.OptionalText(snapshot, "error") ?? Localized.Get("未知错误"))); break;
            default: ShowSummary(Localized.Format("{0}测速完成：成功 {1}，超时 {2}，失败 {3}", prefix, completed, timedOut, failed)); break;
        }
    }

    private Task AddProviderAsync() => ProviderDialogAsync(null);

    private async Task ProviderDialogAsync(JsonElement? provider, bool inline = false)
    {
        var form = new StackPanel { Spacing = 12, MinWidth = 360 };
        var catalog = Json.Field(metadata, "provider_catalog");
        var presets = Json.Rows(Json.Field(catalog, "presets"));
        var protocols = Json.Rows(Json.Field(catalog, "protocols"));
        var preset = Choice(Localized.Get("供应商类型"), presets.Select(row => Json.Text(row, "id")).ToArray(),
            provider is { } stored ? Json.OptionalText(stored, "preset_id") ?? "custom" : Json.Text(catalog, "default_preset"));
        preset.IsEnabled = provider == null;
        var id = Input(Localized.Get("供应商 ID"), provider is { } p ? Json.Text(p, "id") : "");
        id.IsEnabled = provider == null;
        var name = Input(Localized.Get("显示名称"), provider is { } n ? Json.Text(n, "display_name") : "");
        var url = Input(Localized.Get("Base URL（预设可留空）"), provider is { } u ? Json.Text(u, "base_url") : "");
        AutomationProperties.SetAutomationId(url, "provider-base-url");
        var website = Input(Localized.Get("供应商网站（可选）"), provider is { } site ? Json.OptionalText(site, "website_url") ?? "" : "");
        var imagePath = Input(Localized.Get("图像生成路径（可选）"), provider is { } image ? Json.OptionalText(image, "image_generation_path") ?? "" : "");
        var apiPath = Input(Localized.Get("API 路径（custom，可选）"), provider is { } api ? Json.OptionalText(api, "api_path") ?? "" : "");
        var key = new PasswordBox { Header = provider == null ? "API Key" : Localized.Get("新 API Key（留空保留）") };
        var protocol = Choice(Localized.Get("协议（custom）"), new[] { Localized.Get("自动检测") }.Concat(protocols.Select(row => Json.Text(row, "id"))).ToArray(),
            provider is { } pr ? Json.Text(pr, "protocol") : Localized.Get("自动检测"));
        var username = Input(Localized.Get("OneAPI 额度用户名"), provider is { } q ? Json.OptionalText(q, "quota_username") ?? "" : "");
        var workspace = Input("OpenCode Go workspace ID", provider is { } w ? Json.OptionalText(w, "quota_workspace_id") ?? "" : "");
        var cookie = new PasswordBox { Header = Localized.Get("OpenCode Go Cookie（留空保留）") };
        var accessKey = new PasswordBox { Header = Localized.Get("AWS Access Key ID（留空保留）") };
        var secretKey = new PasswordBox { Header = Localized.Get("AWS Secret Access Key（留空保留）") };
        var session = new PasswordBox { Header = Localized.Get("AWS Session Token（可选）") };
        var region = Input("AWS Region", provider is { } r ? Json.OptionalText(r, "aws_region") ?? Json.Text(catalog, "default_aws_region") : Json.Text(catalog, "default_aws_region"));
        var bridge = Choice(Localized.Get("OneAPI 认证桥接"), ["disabled", "ducx_loopback"],
            provider is { } b ? Json.OptionalText(b, "baidu_auth_bridge") ?? "disabled" : "disabled");
        var report = new CheckBox
        {
            Content = Localized.Get("OneAPI 代码上报"),
            IsChecked = provider is { } br &&
            Json.Field(br, "baidu_code_report").ValueKind == JsonValueKind.True
        };
        string[] overrideModels = provider is { } configured
            ? Json.Strings(Json.Field(configured, "selected_models")) : [];
        var reviewOverride = new CheckBox { Content = Localized.Get("Codex 审查模型覆盖"), IsChecked = provider is { } reviewProvider && Json.OptionalText(reviewProvider, "auto_review_model") is { Length: > 0 }, IsEnabled = overrideModels.Length > 0 };
        var reviewModel = Choice(Localized.Get("审查模型"), overrideModels, provider is { } rm ? Json.OptionalText(rm, "auto_review_model") ?? overrideModels.FirstOrDefault() ?? "" : "");
        var titleOverride = new CheckBox { Content = Localized.Get("Codex 会话命名模型覆盖"), IsChecked = provider is { } titleProvider && Json.OptionalText(titleProvider, "codex_thread_title_model") is { Length: > 0 }, IsEnabled = overrideModels.Length > 0 };
        var titleModel = Choice(Localized.Get("会话命名模型"), overrideModels, provider is { } tm ? Json.OptionalText(tm, "codex_thread_title_model") ?? overrideModels.FirstOrDefault() ?? "" : "");
        void OverrideFields()
        {
            reviewOverride.Visibility = titleOverride.Visibility = provider == null ? Visibility.Collapsed : Visibility.Visible;
            reviewModel.Visibility = provider != null && reviewOverride.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
            titleModel.Visibility = provider != null && titleOverride.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
        }
        reviewOverride.Checked += (_, _) => OverrideFields();
        reviewOverride.Unchecked += (_, _) => OverrideFields();
        titleOverride.Checked += (_, _) => OverrideFields();
        titleOverride.Unchecked += (_, _) => OverrideFields();
        OverrideFields();
        var oauthTimeout = Input(Localized.Get("OAuth 授权超时（秒）"), Json.Field(catalog, "oauth_timeout_seconds").ToString());
        var oauthHint = Text(Localized.Get("将在浏览器中授权。关闭网页不会回传取消；可使用窗口的取消操作结束等待。"));
        form.Children.Add(oauthTimeout);
        form.Children.Add(oauthHint);
        foreach (var element in new FrameworkElement[] { preset, id, name, url, website, protocol, apiPath, imagePath, key, username, bridge, report, workspace, cookie, accessKey, secretKey, session, region, reviewOverride, reviewModel, titleOverride, titleModel })
            form.Children.Add(element);
        void Fields()
        {
            var definition = presets.Single(row => Json.Text(row, "id") == (string)preset.SelectedItem);
            var aws = Json.Text(definition, "auth_mode") == "aws_sigv4";
            var oauth = provider == null && Json.Text(definition, "auth_mode") == "local_oauth";
            oauthTimeout.Visibility = oauth ? Visibility.Visible : Visibility.Collapsed;
            oauthHint.Visibility = oauth ? Visibility.Visible : Visibility.Collapsed;
            id.Visibility = oauth && !Json.Flag(definition, "allow_multiple") ? Visibility.Collapsed : Visibility.Visible;
            var localSession = Json.Text(definition, "auth_mode") == "local_session";
            key.Header = Json.Flag(definition, "api_key_required") ? "API Key" : localSession ? Localized.Get("API Key（可选；留空使用原生客户端登录状态）") : Localized.Get("API Key（可选；留空使用本地 OAuth）");
            key.Visibility = aws || oauth ? Visibility.Collapsed : Visibility.Visible;
            foreach (var element in new FrameworkElement[] { name, url, website, imagePath })
                element.Visibility = oauth ? Visibility.Collapsed : Visibility.Visible;
            foreach (var element in new FrameworkElement[] { accessKey, secretKey, session, region }) element.Visibility = aws ? Visibility.Visible : Visibility.Collapsed;
            foreach (var element in new FrameworkElement[] { username, bridge, report }) element.Visibility = Json.Flag(definition, "quota_username_required") ? Visibility.Visible : Visibility.Collapsed;
            foreach (var element in new FrameworkElement[] { workspace, cookie }) element.Visibility = Json.Flag(definition, "quota_credentials_required") ? Visibility.Visible : Visibility.Collapsed;
            protocol.Visibility = Json.Flag(definition, "custom_endpoint") ? Visibility.Visible : Visibility.Collapsed;
            apiPath.Visibility = protocol.Visibility;
        }
        var ducxHint = Text(Localized.Get("DUCX 使用独立副本。保存时自动下载；需要登录时会打开终端显示二维码，请在终端完成扫码。关闭终端或取消后可重新登录。"));
        var setupDucx = Button(Localized.Get("安装 / 登录 DUCX…"), async () =>
        {
            await ExecuteUiAsync(Localized.Get("准备 DUCX，请在弹出的终端扫码"), async token =>
            {
                var output = await ((App)Microsoft.UI.Xaml.Application.Current).Operations.ChangeAsync(
                    ["connect", "ducx", "--json"], token, progress: new Progress<string>(message => StatusText.Text = message));
                using var document = JsonDocument.Parse(output);
                var executable = Json.Text(document.RootElement, "executable");
                if (!Path.IsPathFullyQualified(executable)) throw new CliFailure(Localized.Get("DUCX 返回的路径必须是绝对路径。"));
                ShowNotice(Localized.Get("DUCX 已就绪"), Localized.Get("扫码登录已完成，可以保存服务商配置。"), InfoBarSeverity.Success);
            });
        });
        AutomationProperties.SetAutomationId(setupDucx, "ducx-login");
        form.Children.Add(ducxHint);
        if (inline) form.Children.Add(setupDucx);
        void BridgeFields()
        {
            var visible = bridge.Visibility == Visibility.Visible && (string)bridge.SelectedItem == "ducx_loopback";
            ducxHint.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
            setupDucx.Visibility = ducxHint.Visibility;
        }
        bridge.SelectionChanged += (_, _) => BridgeFields();
        preset.SelectionChanged += (_, _) => { Fields(); BridgeFields(); }; Fields(); BridgeFields();
        AutomationProperties.SetAutomationId(bridge, "provider-auth-bridge");
        if (inline)
            foreach (var control in form.Children.OfType<FrameworkElement>()) TrackDraft(control);
        if (!inline)
        {
            var dialog = Dialog(provider == null ? Localized.Get("新增服务商") : Localized.Get("连接配置"), new ScrollViewer { Content = form, MaxHeight = 480 });
            dialog.PrimaryButtonText = Localized.Get("保存");
            if (await ShowDialogAsync(dialog) != ContentDialogResult.Primary) return;
        }
        async Task Save()
        {
            var args = new List<string> { "provider", provider == null ? "add" : "update" };
            if (provider == null) args.AddRange(["--preset", (string)preset.SelectedItem]);
            if (id.Visibility == Visibility.Visible && !string.IsNullOrWhiteSpace(id.Text))
            {
                if (provider == null) args.Add("--id");
                args.Add(id.Text.Trim());
            }
            var secrets = new Dictionary<string, string>();
            void Secret(string flag, PasswordBox box, string variable)
            {
                if (box.Password.Length == 0) return;
                secrets[variable] = box.Password; args.AddRange([flag, $"@env:{variable}"]);
            }
            foreach (var (flag, box) in new[] { ("--display-name", name), ("--base-url", url), ("--website-url", website), ("--image-generation-path", imagePath) })
                if (box.Visibility == Visibility.Visible && !string.IsNullOrWhiteSpace(box.Text)) args.AddRange([flag, box.Text.Trim()]);
            var definition = presets.Single(row => Json.Text(row, "id") == (string)preset.SelectedItem);
            if (Json.Flag(definition, "custom_endpoint") && (string)protocol.SelectedItem != Localized.Get("自动检测")) args.AddRange(["--protocol", (string)protocol.SelectedItem]);
            if (Json.Flag(definition, "custom_endpoint") && !string.IsNullOrWhiteSpace(apiPath.Text)) args.AddRange(["--api-path", apiPath.Text.Trim()]);
            if (Json.Text(definition, "auth_mode") == "aws_sigv4")
            {
                Secret("--aws-access-key-id", accessKey, "CM_AWS_ID");
                Secret("--aws-secret-access-key", secretKey, "CM_AWS_SECRET");
                Secret("--aws-session-token", session, "CM_AWS_SESSION");
                args.AddRange(["--aws-region", region.Text.Trim()]);
            }
            else if (key.Visibility == Visibility.Visible) Secret("--key", key, "CM_API_KEY");
            if (oauthTimeout.Visibility == Visibility.Visible && !string.IsNullOrWhiteSpace(oauthTimeout.Text))
                args.AddRange(["--oauth-timeout-seconds", oauthTimeout.Text.Trim()]);
            if (Json.Flag(definition, "quota_username_required"))
            {
                if (!string.IsNullOrWhiteSpace(username.Text)) args.AddRange(["--quota-username", username.Text.Trim()]);
                args.AddRange(["--baidu-auth-bridge", (string)bridge.SelectedItem, "--baidu-code-report", report.IsChecked == true ? "true" : "false"]);
            }
            if (Json.Flag(definition, "quota_credentials_required"))
            {
                if (!string.IsNullOrWhiteSpace(workspace.Text)) args.AddRange(["--quota-workspace-id", workspace.Text.Trim()]);
                Secret("--quota-auth-cookie", cookie, "CM_QUOTA_COOKIE");
            }
            if (provider != null)
            {
                if (reviewOverride.IsChecked == true && reviewModel.SelectedItem is string reviewSelection && !string.IsNullOrEmpty(reviewSelection))
                    args.AddRange(["--auto-review-model", reviewSelection]);
                else args.Add("--clear-auto-review-model");
                if (titleOverride.IsChecked == true && titleModel.SelectedItem is string titleSelection && !string.IsNullOrEmpty(titleSelection))
                    args.AddRange(["--codex-thread-title-model", titleSelection]);
                else args.Add("--clear-codex-thread-title-model");
            }
            try { await ChangeAsync(Localized.Get("保存供应商"), args.ToArray(), apply: true, secrets: secrets); }
            finally { secrets.Clear(); }
        }
        if (inline && provider is { } connection)
        {
            var body = new StackPanel { Spacing = 16, MaxWidth = 720, HorizontalAlignment = HorizontalAlignment.Stretch };
            body.Children.Add(Card(Localized.Get("连接配置"), Localized.Get("密钥已配置时留空保留；修改后使用底部「应用更改」。"), form));
            body.Children.Add(Button(Localized.Get("清除密钥"), () => ChangeAsync(Localized.Get("清除密钥"), ["provider", "update", Json.Text(connection, "id"), Json.OptionalText(connection, "preset_id") == "aws-bedrock" ? "--clear-aws-credentials" : "--clear-key"], apply: true)));
            pendingApply = Save;
            connectionDrafts[Json.Text(connection, "id")] = (body, Save);
            PageBody.Children.Add(body);
        }
        else await Save();

    }

    private async Task AgentsAsync(CancellationToken token)
    {
        var revision = ((App)Microsoft.UI.Xaml.Application.Current).Operations.Revision;
        if (agentSnapshot == null || agentRevision != revision || Stopwatch.GetElapsedTime(agentSnapshotStamp) >= ReadSnapshotAge)
        {
            agentSnapshot = Json.Rows(Json.Field(await Client.QueryAsync(["agent", "list", "--json"], token), "agents"));
            agentSnapshotAt = DateTimeOffset.UtcNow;
            agentSnapshotStamp = Stopwatch.GetTimestamp();
            agentRevision = revision;
        }
        var agents = agentSnapshot;
        var checkVersions = Button(Localized.Get("检查版本更新"), () => ExecuteUiAsync(Localized.Get("检查 Agent 版本"), async cancellation =>
        {
            var versions = Json.Rows(Json.Field(await Client.QueryAsync(["agent", "versions", "--json", "--refresh"], cancellation), "agents"));
            var body = new StackPanel { Spacing = 12 };
            foreach (var agent in versions)
            {
                var version = Json.Field(agent, "cli");
                var id = Json.Text(agent, "id");
                var row = Row();
                row.Children.Add(Text(Localized.Format("{0} · 当前 {1} · 最新 {2}", id, Json.OptionalText(version, "version") ?? Json.OptionalText(version, "app_version") ?? Localized.Get("未知"), Json.OptionalText(version, "latest") ?? Localized.Get("未知"))));
                if (Json.Flag(version, "update")) row.Children.Add(Button(Localized.Get("升级"), () => ChangeAsync(Localized.Get("升级 Agent"), ["agent", "upgrade", id, "--json"])));
                if (Json.OptionalText(version, "error") is { } error) row.Children.Add(Text(error));
                body.Children.Add(row);
            }
            PageBody.Children.Insert(1, Card(Localized.Get("版本检查结果"), "", body));
        }, readOnly: true));
        checkVersions.Content = IconLabel("\uE895", Localized.Get("检查版本更新"));
        checkVersions.Style = Themed("FlatButton");
        var detectedCount = agents.Count(agent => Json.Flag(agent, "detected"));
        var connectedCount = agents.Count(agent => Json.Text(Json.Field(agent, "integration"), "state") is "managed" or "drift");
        var agentToolbar = new Grid();
        var agentSummary = new TextBlock { Text = Localized.Format("已接入 {0} 个 Agent · 已检测到 {1} 个 · {2:HH:mm:ss}", connectedCount, detectedCount, agentSnapshotAt.ToLocalTime()), Style = Themed("Caption"), VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetAutomationId(agentSummary, "agent-summary");
        agentToolbar.Children.Add(agentSummary);
        checkVersions.HorizontalAlignment = HorizontalAlignment.Right;
        agentToolbar.Children.Add(checkVersions);
        PageBody.Children.Add(agentToolbar);
        var undetected = new StackPanel { Spacing = 0 };
        var detectedRows = new StackPanel { Spacing = 0 };
        foreach (var agent in agents)
        {
            var id = Json.Text(agent, "id");
            var integration = Json.Field(agent, "integration");
            var state = Json.Text(integration, "state");
            var stateLabel = state switch
            {
                "managed" => Localized.Get("已接入"),
                "unmanaged" => Localized.Get("未接入"),
                "drift" => Localized.Get("配置已改变"),
                "error" => Localized.Get("检测失败"),
                "unsupported" => Localized.Get("暂不支持接入"),
                "pending" => Localized.Get("等待原生应用确认"),
                "pending_removal" => Localized.Get("等待原生应用移除"),
                _ => Localized.Format("未知状态：{0}", state)
            };
            var version = Json.Field(agent, "cli");
            var integrationMode = Json.OptionalText(integration, "mode");
            var modeLabel = integrationMode switch
            {
                "native_models" => Localized.Get("厂商原生模型管理"),
                "native_credentials" => Localized.Get("原生凭据存储"),
                "native_runtime" => Localized.Get("原生运行时"),
                "native_app" => Localized.Get("原生应用 API"),
                "confirmed_import" => Localized.Get("应用内确认导入"),
                _ => ""
            };
            if (integrationMode == "native_models" && state == "managed")
                stateLabel = Localized.Get("原生模型已托管");
            if (modeLabel.Length > 0) stateLabel += $" · {modeLabel}";
            var connected = state is "managed" or "drift" or "pending" or "pending_removal";
            var connectSwitch = new ToggleSwitch { IsOn = connected, OnContent = "", OffContent = "", IsEnabled = state != "unsupported", MinWidth = 44, VerticalAlignment = VerticalAlignment.Center };
            AutomationProperties.SetName(connectSwitch, Localized.Format("{0} 接入", Json.Text(agent, "name")));
            var restoringSwitch = false;
            connectSwitch.Toggled += async (_, _) =>
            {
                if (restoringSwitch || busy) return;
                var connect = connectSwitch.IsOn;
                restoringSwitch = true; connectSwitch.IsOn = connected; restoringSwitch = false;
                try
                {
                    if (!connect)
                    {
                        if (await ConfirmAsync(Localized.Get("断开 Agent？"), Localized.Get("CLI 将恢复或移除此 Agent 的托管配置。"), Localized.Get("断开")))
                            await ChangeAsync(Localized.Get("断开 Agent"), ["agent", "disconnect", id]);
                        return;
                    }
                    var args = new List<string> { "agent", "connect", id };
                    if (id == "codex")
                    {
                        var mode = Choice(Localized.Get("接入模式"), [Localized.Get("官方账号 + 自定义模型"), Localized.Get("仅自定义模型")], Localized.Get("官方账号 + 自定义模型"));
                        var dialog = Dialog(Localized.Get("接入 Codex"), mode); dialog.PrimaryButtonText = Localized.Get("接入");
                        if (await ShowDialogAsync(dialog) != ContentDialogResult.Primary) return;
                        args.AddRange(["--mode", mode.SelectedIndex == 0 ? "official-account" : "custom-only"]);
                    }
                    await ChangeAsync(Localized.Get("接入 Agent"), args.ToArray());
                }
                catch (Exception error) { ShowNotice(Localized.Get("Agent 接入失败"), error.Message, InfoBarSeverity.Error); }
            };
            var heading = new Grid { ColumnSpacing = 12 };
            heading.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            heading.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var title = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
            title.Children.Add(new TextBlock { Text = Json.Text(agent, "name"), FontSize = 15 });
            var versions = string.Join("  ", new[] { Json.OptionalText(version, "version") is { } cliVersion ? (Json.OptionalText(version, "app_version") != null ? $"CLI {cliVersion}" : cliVersion) : null,
                Json.OptionalText(version, "app_version") is { } appVersion ? $"App {appVersion}" : null }.Where(value => value != null));
            if (versions.Length > 0) title.Children.Add(new TextBlock { Text = versions, Style = Themed("Caption"), FontSize = 14, VerticalAlignment = VerticalAlignment.Center });
            var text = new StackPanel { Spacing = 3, VerticalAlignment = VerticalAlignment.Center };
            text.Children.Add(title);
            var summary = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
            summary.Children.Add(new Microsoft.UI.Xaml.Shapes.Ellipse { Style = Themed(connected ? "DotOn" : state == "error" ? "DotWarn" : "DotOff"), VerticalAlignment = VerticalAlignment.Center });
            var problem = Json.OptionalText(integration, "error") ?? (Json.OptionalText(version, "error") is { } failure ? Localized.Format("版本检查失败：{0}", failure) : null);
            summary.Children.Add(new TextBlock { Text = stateLabel, Style = Themed("Caption"), FontSize = 13, MaxWidth = 760 });
            if (connected && !Json.Flag(agent, "detected"))
                summary.Children.Add(new TextBlock { Text = Localized.Get("未检测到安装程序"), Style = Themed("Caption"), FontSize = 13 });
            ToolTipService.SetToolTip(connectSwitch, problem ?? Json.Text(integration, "detail"));
            if (state != "unsupported") text.Children.Add(summary);
            var identity = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 16 };
            identity.Children.Add(new Border { Width = 40, Height = 40, Child = Marks.Agent(id) }); identity.Children.Add(text);
            heading.Children.Add(identity);
            var controls = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
            if (Json.Flag(version, "update"))
                controls.Children.Add(Button(Localized.Format("更新到 {0}", Json.Text(version, "latest")), () => ChangeAsync(Localized.Get("升级 Agent"), ["agent", "upgrade", id, "--json"])));
            controls.Children.Add(connectSwitch);
            Grid.SetColumn(controls, 1); heading.Children.Add(controls);
            var rows = connected || Json.Flag(agent, "detected") ? detectedRows : undetected;
            var entry = new Border { Child = heading, Padding = new Thickness(20, 14, 20, 14), Style = Themed(rows.Children.Count == 0 ? "RowPlain" : "RowDivider") };
            // Dividers sit above every row except the first, so the last row has no trailing line.
            if (rows.Children.Count > 0) entry.BorderThickness = new Thickness(0, 1, 0, 0);
            rows.Children.Add(entry);
        }
        if (detectedRows.Children.Count > 0) PageBody.Children.Add(new Border { Child = detectedRows, Style = Themed("ListCard"), MaxWidth = 1120 });
        if (undetected.Children.Count > 0)
        {
            var section = new Expander { Header = Localized.Format("显示未接入且未检测到的 {0} 个 Agent", undetected.Children.Count), Content = undetected, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch, MaxWidth = 1120 };
            AutomationProperties.SetAutomationId(section, "agent-undetected");
            PageBody.Children.Add(section);
        }
    }

    private async Task UsageAsync(CancellationToken token)
    {
        var range = Choice(Localized.Get("统计范围"), [Localized.Get("今天"), Localized.Get("7 天"), Localized.Get("30 天")], Localized.Get("7 天"));
        var metric = Choice(Localized.Get("图表指标"), ["Tokens", Localized.Get("请求次数")], "Tokens");
        var grouping = Choice(Localized.Get("图表分组"), [Localized.Get("构成"), Localized.Get("按 Provider 拆分")], Localized.Get("构成"));
        foreach (var picker in new[] { range, metric, grouping }) picker.MinWidth = 150;
        var filters = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
        filters.Children.Add(range); filters.Children.Add(metric); filters.Children.Add(grouping);
        var usageBody = new StackPanel { Spacing = 12 };
        JsonElement[] daily = [], previous = [], turns = [];
        JsonElement[]? slotTurns = null;
        UsageSlot[] slots = [];
        UsageSlot? slot = null;
        Dictionary<string, string> providerNames = [];
        string? session = null;
        var expanded = new HashSet<string>();
        int Days() => range.SelectedIndex switch { 0 => 1, 2 => 30, _ => 7 };

        var status = Choice(Localized.Get("状态"), [Localized.Get("全部"), Localized.Get("成功"), Localized.Get("失败")], Localized.Get("全部"));
        var model = Choice(Localized.Get("模型"), [Localized.Get("全部模型")], Localized.Get("全部模型"));
        status.MinWidth = 120; model.MinWidth = 200; status.HorizontalAlignment = model.HorizontalAlignment = HorizontalAlignment.Left;
        var substituted = new CheckBox { Content = Localized.Get("仅看模型替换"), VerticalAlignment = VerticalAlignment.Bottom };
        var grouped = new CheckBox { Content = Localized.Get("按会话分组"), VerticalAlignment = VerticalAlignment.Bottom };
        AutomationProperties.SetAutomationId(status, "usage-status"); AutomationProperties.SetAutomationId(model, "usage-model");
        AutomationProperties.SetAutomationId(grouped, "usage-group-session");
        var chips = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Bottom };
        var tableBar = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
        foreach (var element in new FrameworkElement[] { status, model, substituted, grouped, chips }) tableBar.Children.Add(element);
        var updatingFilters = false;

        var summaryHost = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        var chartsHost = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        var turnsHost = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        var tableNote = new TextBlock { Style = Themed("SecondaryText") };
        var tableBody = new StackPanel { Spacing = 12 };
        tableBody.Children.Add(tableBar); tableBody.Children.Add(turnsHost);
        var tableCard = Card(Localized.Get("回复明细"), "", tableBody);
        ((StackPanel)tableCard.Child).Children.Insert(1, tableNote);
        void Attach()
        {
            usageBody.Children.Clear();
            usageBody.Children.Add(summaryHost); usageBody.Children.Add(chartsHost); usageBody.Children.Add(tableCard);
        }
        Attach();

        Button Chip(string text, Action clear)
        {
            var chip = new Button { Content = IconLabel("\uE711", text), Padding = new Thickness(8, 3, 8, 3), FontSize = 12 };
            AutomationProperties.SetName(chip, Localized.Format("清除筛选：{0}", text));
            chip.Click += (_, _) => clear();
            return chip;
        }
        void RenderTurns()
        {
            var scoped = (slot == null ? turns : slotTurns ?? [])
                .Where(turn => session == null || Json.Text(turn, "session") == session).ToArray();
            var substitutions = scoped.Count(UsageStats.IsSubstituted);
            substituted.Visibility = substitutions > 0 ? Visibility.Visible : Visibility.Collapsed;
            substituted.Content = Localized.Format("仅看模型替换（{0}）", substitutions);
            if (substitutions == 0) substituted.IsChecked = false;
            var failures = scoped.Count(UsageStats.IsFailed);
            updatingFilters = true;
            var statusIndex = Math.Max(0, status.SelectedIndex);
            status.Items.Clear();
            foreach (var label in new[] { Localized.Format("全部（{0}）", scoped.Length), Localized.Format("成功（{0}）", scoped.Length - failures), Localized.Format("失败（{0}）", failures) }) status.Items.Add(label);
            status.SelectedIndex = statusIndex;
            updatingFilters = false;
            var modelId = model.SelectedIndex > 0 ? (string)model.SelectedItem : null;
            var shown = scoped.Where(turn => status.SelectedIndex switch
                {
                    1 => !UsageStats.IsFailed(turn),
                    2 => UsageStats.IsFailed(turn),
                    _ => true
                })
                .Where(turn => modelId == null || Json.Text(turn, "upstream_model_id") == modelId)
                .Where(turn => substituted.IsChecked != true || UsageStats.IsSubstituted(turn)).ToArray();
            chips.Children.Clear();
            if (slot != null) chips.Children.Add(Chip(UsageView.SlotTitle(slot), () => { slot = null; slotTurns = null; RenderCharts(); RenderTurns(); }));
            if (session != null)
            {
                var title = scoped.Select(turn => Json.OptionalText(turn, "session_display")).FirstOrDefault(name => name != null) ?? session;
                chips.Children.Add(Chip(Localized.Format("会话 {0}", title), () => { session = null; RenderTurns(); }));
            }
            tableNote.Text = slot != null && slotTurns == null ? Localized.Get("正在读取这段时间的回复…")
                : Localized.Format("显示 {0} 条回复", shown.Length) + (slot == null && session == null && turns.Length >= 500 ? Localized.Get("（只列出最近 500 条；点击图表柱子查看某段时间的全部回复）") : "");
            if (shown.Length == 0) { turnsHost.Content = Text(slot != null && slotTurns == null ? "" : Localized.Get("没有符合条件的回复。")); return; }
            var rows = UsageView.Rows(shown, grouped.IsChecked == true, expanded);
            var table = new StackPanel();
            table.Children.Add(UsageView.TableHeader());
            table.Children.Add(UsageView.Turns(rows, providerNames,
                tag => { session = session == tag ? null : tag; RenderTurns(); },
                tag => { if (!expanded.Remove(tag)) expanded.Add(tag); RenderTurns(); }));
            turnsHost.Content = new Border { Child = table, Style = Themed("ListCard") };
        }
        async Task SelectSlot(UsageSlot? selected)
        {
            slot = selected; slotTurns = null;
            RenderCharts(); RenderTurns();
            if (selected == null) return;
            try
            {
                var loaded = Json.Rows(await Client.QueryAsync(["usage", "--turns", "--json", "--limit", "5000",
                    "--since-ms", selected.Start.ToUnixTimeMilliseconds().ToString(System.Globalization.CultureInfo.InvariantCulture),
                    "--until-ms", selected.End.ToUnixTimeMilliseconds().ToString(System.Globalization.CultureInfo.InvariantCulture)], lifetime.Token));
                if (!ReferenceEquals(slot, selected)) return;
                slotTurns = loaded;
                RenderTurns();
            }
            catch (CliFailure error) when (ReferenceEquals(slot, selected))
            {
                slot = null; RenderCharts(); RenderTurns();
                ShowNotice(Localized.Get("读取回复失败"), error.Message, InfoBarSeverity.Error);
            }
        }
        void RenderCharts()
        {
            var chart = UsageView.Chart(slots, providerNames, metric.SelectedIndex == 1, grouping.SelectedIndex == 1, slot, selected => _ = SelectSlot(selected));
            var card = Card(Days() == 1 ? Localized.Get("用量趋势 · 按小时") : Localized.Get("用量趋势 · 按天（UTC）"), "", chart);
            chartsHost.Content = card;
        }
        async Task ReadUsage(CancellationToken cancellation)
        {
            pageLoaded = false;
            var days = Days();
            string Arg(int value) => value.ToString(System.Globalization.CultureInfo.InvariantCulture);
            try
            {
                var snapshots = await Task.WhenAll(
                    Client.QueryAsync(["usage", "--daily", "--json", "--days", Arg(days * 2)], cancellation),
                    Client.QueryAsync(["usage", "--turns", "--json", "--days", Arg(days), "--limit", "500"], cancellation),
                    Client.QueryAsync(["providers", "list", "--json"], cancellation));
                var now = DateTimeOffset.Now;
                (daily, previous) = UsageBuckets.SplitPeriods(Json.Rows(snapshots[0]), days, now);
                var firstMs = (UsageBuckets.Today(now) - days + 1) * UsageBuckets.DayMs;
                turns = Json.Rows(snapshots[1]).Where(turn => turn.TryGetProperty("at_ms", out var at) && at.GetInt64() >= firstMs).ToArray();
                slots = UsageBuckets.Slots(daily, turns, days, now);
                var listedProviders = Json.Rows(Json.Field(snapshots[2], "providers"));
                var byName = listedProviders.GroupBy(provider => Json.Text(provider, "display_name"))
                    .ToDictionary(group => group.Key, group => group.Count());
                providerNames = listedProviders.ToDictionary(provider => Json.Text(provider, "id"), provider =>
                {
                    var name = Json.Text(provider, "display_name");
                    return byName[name] == 1 ? name : $"{name} ({Json.Text(provider, "id")})";
                });
                session = null; slot = null; slotTurns = null;
                updatingFilters = true;
                var models = turns.Select(turn => Json.Text(turn, "upstream_model_id")).Distinct().Order(StringComparer.Ordinal).ToArray();
                var keep = model.SelectedIndex > 0 && models.Contains((string)model.SelectedItem) ? (string)model.SelectedItem : null;
                model.Items.Clear(); model.Items.Add(Localized.Get("全部模型"));
                foreach (var id in models) model.Items.Add(id);
                model.SelectedItem = keep ?? Localized.Get("全部模型");
                updatingFilters = false;
                if (usageBody.Children.Count != 3) Attach();
                summaryHost.Content = UsageView.Summary(UsageStats.Sum(daily.Select(UsageStats.Daily)), UsageStats.Sum(previous.Select(UsageStats.Daily)), slots);
                RenderCharts(); RenderTurns();
                pageLoaded = true;
            }
            catch (CliFailure) when (cancellation.IsCancellationRequested) { throw; }
            catch (CliFailure error) { usageBody.Children.Clear(); usageBody.Children.Add(Card(Localized.Get("用量不可用"), error.Message)); }
        }
        range.SelectionChanged += async (_, _) => await ExecuteUiAsync(Localized.Get("读取用量"), ReadUsage, readOnly: true);
        metric.SelectionChanged += (_, _) => RenderCharts();
        grouping.SelectionChanged += (_, _) => RenderCharts();
        status.SelectionChanged += (_, _) => { if (!updatingFilters) RenderTurns(); };
        model.SelectionChanged += (_, _) => { if (!updatingFilters) RenderTurns(); };
        substituted.Click += (_, _) => RenderTurns();
        grouped.Click += (_, _) => RenderTurns();
        PageBody.Children.Add(filters); PageBody.Children.Add(usageBody);
        await ReadUsage(token);
    }

    private static Task OpenPathAsync(string path)
    {
        if (!Path.IsPathFullyQualified(path) || !Directory.Exists(path)) throw new DirectoryNotFoundException(Localized.Format("目录不存在：{0}", path));
        Process.Start(new ProcessStartInfo(path) { UseShellExecute = true });
        return Task.CompletedTask;
    }

    private Button Button(string label, Func<Task> work, bool primary = false)
    {
        var button = new Button { Content = label };
        if (primary) button.Style = (Style)Microsoft.UI.Xaml.Application.Current.Resources["AccentButtonStyle"];
        button.Click += async (_, _) =>
        {
            try { await work(); }
            catch (Exception error) { ShowNotice(Localized.Format("{0}失败", label), error.Message, InfoBarSeverity.Error); }
        };
        return button;
    }

    private static ActionPanel Row() => new();
    private static TextBlock Text(string value) => new() { Text = value, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
    private static TextBox Input(string header, string value) => new() { Header = header, Text = value };
    private static ComboBox Choice(string header, string[] values, string selected)
    {
        var picker = new ComboBox { Header = header, HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var value in values) picker.Items.Add(value);
        // Keep unavailable saved references visible rather than silently replacing them.
        if (!values.Contains(selected)) picker.Items.Add(selected);
        picker.SelectedItem = selected;
        return picker;
    }

    private Border Card(string title, string description, UIElement? content = null)
    {
        var body = new StackPanel { Spacing = 12 };
        body.Children.Add(new TextBlock { Text = title, Style = Themed("BodyStrongTextBlockStyle"), FontSize = 16, TextWrapping = TextWrapping.Wrap });
        if (!string.IsNullOrWhiteSpace(description))
            body.Children.Add(new TextBlock
            {
                Text = description,
                Style = (Style)Microsoft.UI.Xaml.Application.Current.Resources["SecondaryText"]
            });
        if (content != null) body.Children.Add(content);
        return new Border { Style = (Style)Microsoft.UI.Xaml.Application.Current.Resources["Card"], Child = body };
    }

    private ContentDialog Dialog(string title, UIElement content) => new()
    {
        Title = title,
        Content = content,
        CloseButtonText = Localized.Get("取消"),
        XamlRoot = Root.XamlRoot,
        RequestedTheme = Root.ActualTheme,
        DefaultButton = ContentDialogButton.Primary
    };

    private async Task<ContentDialogResult> ShowDialogAsync(ContentDialog dialog)
    {
        if (dialogOpen) return ContentDialogResult.None;
        dialogOpen = true;
        try { return await dialog.ShowAsync(); }
        finally { dialogOpen = false; }
    }

    private async Task<bool> ConfirmAsync(string title, string message, string confirm)
    {
        var dialog = Dialog(title, Text(message)); dialog.PrimaryButtonText = confirm;
        return await ShowDialogAsync(dialog) == ContentDialogResult.Primary;
    }

    private void ShowNotice(string title, string message, InfoBarSeverity severity)
    {
        Notice.Title = title; Notice.Message = message; Notice.Severity = severity; Notice.IsOpen = true;
    }
}
