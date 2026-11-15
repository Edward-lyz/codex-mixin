using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.Primitives;
using Avalonia.Input;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Threading;
using TKCode.Arc;
using TKCode.Arc.DesignTokens;
using TKCode.Arc.Motion;
using ArcButton = TKCode.Arc.Button;
using Avalonia.Automation;

namespace CodexMixin;

internal sealed partial class MainWindow : ArcWindow
{
    private readonly CancellationTokenSource lifetime = new();
    private readonly CancellationToken windowToken;
    private readonly CliClient cli;
    private readonly Operations operations;
    private readonly Sidebar navigation;
    private readonly ContentControl pageHost = new() { HorizontalContentAlignment = HorizontalAlignment.Stretch };
    private readonly TextBlock heading = new() { FontSize = 28, FontWeight = FontWeight.SemiBold };
    private readonly TextBlock subtitle = new() { FontSize = 13, TextWrapping = TextWrapping.Wrap };
    private readonly TextBlock notice = new() { TextWrapping = TextWrapping.Wrap, IsVisible = false };
    private readonly TextBlock footer = new() { Text = "CLI · Rust", FontSize = 12 };
    private readonly ArcButton refresh = new() { Content = Localized.Get("刷新"), Variant = ButtonVariant.Secondary };
    private CancellationTokenSource? pageQuery;
    private string currentPage = "providers";
    private bool ready;
    private bool changing;
    private bool draftDirty;
    private bool selectingNavigation;
    private int dialogs;

    internal MainWindow()
    {
        windowToken = lifetime.Token;
        AutomationProperties.SetAutomationId(notice, "operation-status");
        AutomationProperties.SetAutomationId(refresh, "shell-refresh");
        cli = CliClient.Locate();
        operations = new Operations(cli);
        Title = "Codex Mixin";
        Width = 1180;
        Height = 800;
        MinWidth = 820;
        MinHeight = 580;
        WindowStartupLocation = WindowStartupLocation.CenterScreen;
        ArcTokens.Bind(this, BackgroundProperty, ArcDesignTokenKind.Background);
        ArcTokens.Bind(this, ForegroundProperty, ArcDesignTokenKind.Foreground);
        ArcTokens.Bind(subtitle, TextBlock.ForegroundProperty, ArcDesignTokenKind.TextSecondary);
        ArcTokens.Bind(footer, TextBlock.ForegroundProperty, ArcDesignTokenKind.TextMuted);

        var brand = new StackPanel { Spacing = 3, Margin = new Thickness(8, 0) };
        brand.Children.Add(new TextBlock { Text = "MIXIN", FontSize = 18, FontWeight = FontWeight.SemiBold });
        brand.Children.Add(new TextBlock { Text = Localized.UseEnglish ? "Your models. One gateway." : "连接模型，专注创造。", FontSize = 11 });
        navigation = new Sidebar { Header = brand, Footer = footer, ExpandedWidth = 228, IsFilterVisible = true };
        var models = new SidebarSection { Header = Localized.UseEnglish ? "WORKSPACE" : "工作空间" };
        foreach (var (key, label, icon) in new[]
        {
            ("providers", "模型与服务", ArcIcons.Layers),
            ("agents", "Agent 接入", ArcIcons.Terminal),
            ("routing", "模型路由", ArcIcons.GitBranch),
            ("context", "上下文引擎", ArcIcons.Database),
            ("usage", "用量统计", ArcIcons.Activity)
        }) models.Items.Add(new SidebarItem { Key = key, Label = Localized.Get(label), Icon = icon });
        navigation.Sections.Add(models);
        var system = new SidebarSection { Header = Localized.UseEnglish ? "SYSTEM" : "系统" };
        system.Items.Add(new SidebarItem { Key = "settings", Label = Localized.Get("设置与诊断"), Icon = ArcIcons.Settings });
        navigation.Sections.Add(system);
        navigation.SelectedKey = currentPage;
        navigation.SelectionChanged += async (_, args) =>
        {
            if (!ready || selectingNavigation || args.NewKey is not { } key) return;
            if (draftDirty && !await ConfirmAsync(Localized.Get("放弃更改"), Localized.Get("有尚未保存的更改，是否放弃？")))
            {
                selectingNavigation = true;
                navigation.SelectedKey = currentPage;
                selectingNavigation = false;
                return;
            }
            draftDirty = false;
            currentPage = key;
            await RefreshPageAsync();
        };

        var actions = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        var motion = new ArcButton { Content = Localized.UseEnglish ? "Motion" : "动效", Variant = ButtonVariant.Ghost };
        motion.Click += (_, _) =>
        {
            var reduced = ArcMotion.GetLevel(this) == ArcMotionLevel.Reduced;
            ArcMotion.SetLevel(this, reduced ? ArcMotionLevel.Full : ArcMotionLevel.Reduced);
            motion.Content = reduced ? (Localized.UseEnglish ? "Motion" : "动效") : (Localized.UseEnglish ? "Reduced motion" : "减少动态效果");
        };
        actions.Children.Add(motion);
        actions.Children.Add(new ThemeSwitcher { IsIconOnly = true, Variant = ThemeSwitcherVariant.Reveal });
        actions.Children.Add(refresh);
        var menu = new DropdownMenu { Label = Localized.Get("更多") };
        AutomationProperties.SetAutomationId(menu, "shell-menu");
        var quit = new MenuEntry { Header = Localized.Get("退出界面") };
        AutomationProperties.SetAutomationId(quit, "shell-quit");
        quit.Click += async (_, _) => await ((App)Application.Current!).ExitAllAsync();
        menu.Items.Add(quit);
        actions.Children.Add(menu);
        if (TitleBar != null) TitleBar.RightContent = actions;
        refresh.Click += async (_, _) => await RefreshPageAsync();
        var page = new StackPanel { Margin = new Thickness(32, 24, 32, 36), Spacing = 20 };
        var title = new StackPanel { Spacing = 6 };
        title.Children.Add(heading);
        title.Children.Add(subtitle);
        page.Children.Add(title);
        page.Children.Add(notice);
        page.Children.Add(pageHost);
        var scroll = new ScrollViewer { Content = page, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled };
        var root = new DockPanel();
        DockPanel.SetDock(navigation, Dock.Left);
        root.Children.Add(navigation);
        root.Children.Add(scroll);
        Content = root;

        KeyDown += async (_, args) =>
        {
            var modifier = OperatingSystem.IsMacOS() ? KeyModifiers.Meta : KeyModifiers.Control;
            if ((args.KeyModifiers & modifier) == 0) return;
            if (args.Key is >= Key.D1 and <= Key.D6)
            {
                args.Handled = true;
                navigation.SelectedKey = new[] { "providers", "agents", "routing", "context", "usage", "settings" }[(int)args.Key - (int)Key.D1];
            }
            if (args.Key == Key.R) { args.Handled = true; await RefreshPageAsync(); }
            if (args.Key == Key.B) { args.Handled = true; navigation.ToggleCollapsed(); }
            if (args.Key == Key.OemComma) { args.Handled = true; navigation.SelectedKey = "settings"; }
        };
        Opened += async (_, _) =>
        {
            try
            {
                var metadata = await cli.QueryAsync(["interface", "--json"], windowToken);
                Json.Interface(metadata);
                footer.Text = $"Mixin {Json.Text(metadata, "version")} · Rust";
                ready = true;
                await RefreshPageAsync();
                if (!notice.IsVisible) { notice.Text = Localized.Get("就绪"); notice.IsVisible = true; }
            }
            catch (Exception error) when (error is CliFailure or IOException or OperationCanceledException)
            { notice.Text = error.Message; notice.IsVisible = true; }
        };
        Closed += (_, _) =>
        {
            lifetime.Cancel();
            pageQuery?.Cancel();
            pageQuery?.Dispose();
            lifetime.Dispose();
        };
    }

    internal void OpenPage(string key)
    {
        if (key is not ("providers" or "agents" or "routing" or "context" or "usage" or "settings")) return;
        if (!ready) currentPage = key;
        if (!IsVisible) Show();
        WindowState = WindowState.Normal;
        Activate();
        if (ready && navigation.SelectedKey == key) _ = RefreshPageAsync();
        else navigation.SelectedKey = key;
    }

    internal bool HasDraftChanges => draftDirty;
    internal bool HasPendingChanges => changing;
    internal Task<bool> ConfirmExitAsync() => changing ? Task.FromResult(false) : !draftDirty
        ? Task.FromResult(true) : ConfirmAsync(Localized.Get("放弃更改"), Localized.Get("有尚未保存的更改，是否放弃？"));

    internal async Task RunGatewayActionAsync(string action)
    {
        OpenPage("settings");
        await RunActionAsync(["service", action]);
    }

    private async Task RefreshPageAsync()
    {
        if (!ready || changing || !IsVisible) return;
        if (draftDirty)
        {
            if (!await ConfirmAsync(Localized.Get("放弃更改"), Localized.Get("刷新将丢弃尚未保存的更改，是否继续？"))) return;
            draftDirty = false;
        }
        pageQuery?.Cancel();
        pageQuery?.Dispose();
        var request = CancellationTokenSource.CreateLinkedTokenSource(windowToken);
        pageQuery = request;
        var key = currentPage;
        heading.Text = navigation.SelectedItem?.Label ?? "Codex Mixin";
        subtitle.Text = key switch
        {
            "providers" => Localized.UseEnglish ? "Connect providers and choose the models available to your agents." : "连接供应商，为 Agent 选择可用模型。",
            "agents" => Localized.UseEnglish ? "Connect your tools to the same local gateway." : "让常用工具共享同一个本地模型网关。",
            "routing" => Localized.UseEnglish ? "Compose failover, fusion and scheduled model routes." : "编排故障转移、模型融合与定时路由。",
            "context" => Localized.UseEnglish ? "Keep the original conversation. Compress what your model sees." : "归档保留原文，按需压缩模型上下文。",
            "usage" => Localized.UseEnglish ? "Explore requests, tokens, cache hits and costs." : "查看请求、Token、缓存命中与费用。",
            _ => Localized.UseEnglish ? "Manage the gateway, diagnostics and local settings." : "管理网关、诊断与本地设置。"
        };
        refresh.IsEnabled = false;
        notice.IsVisible = false;
        try
        {
            var result = key switch
            {
                "providers" => await BuildProvidersAsync(request.Token),
                "agents" => await BuildAgentsAsync(request.Token),
                "routing" => await BuildRoutingAsync(request.Token),
                "context" => await BuildContextAsync(request.Token),
                "usage" => await BuildUsageAsync(request.Token),
                _ => await BuildSettingsAsync(request.Token)
            };
            request.Token.ThrowIfCancellationRequested();
            if (key == "settings" && result is StackPanel settings) settings.Children.Insert(0, BuildApplicationPreferences());
            if (ReferenceEquals(pageQuery, request)) { pageHost.Content = result; draftDirty = false; }
        }
        catch (OperationCanceledException) { }
        catch (Exception error) when (error is CliFailure or IOException or InvalidOperationException or FormatException)
        {
            if (ReferenceEquals(pageQuery, request))
            { notice.Text = error.Message; notice.IsVisible = true; }
        }
        finally
        {
            if (ReferenceEquals(pageQuery, request)) refresh.IsEnabled = true;
        }
    }

    private async Task<bool> RunActionAsync(string[] args, bool apply = false, IReadOnlyDictionary<string, string>? secrets = null)
    {
        if (changing) return false;
        var succeeded = false;
        changing = true;
        navigation.IsEnabled = false;
        refresh.IsEnabled = false;
        pageHost.IsEnabled = false;
        notice.Text = Localized.Get("正在执行…");
        notice.IsVisible = true;
        try
        {
            await operations.ChangeAsync(args, windowToken, secrets,
                new Progress<string>(text => notice.Text = text), apply);
            notice.Text = Localized.Get("操作完成");
            succeeded = true;
            draftDirty = false;
        }
        catch (Exception error) when (error is CliFailure or IOException or OperationCanceledException)
        {
            if (!windowToken.IsCancellationRequested)
                await ShowDialogAsync(Localized.Get("操作失败"), new TextBlock
                { Text = error.Message, TextWrapping = TextWrapping.Wrap, MaxWidth = 480 });
        }
        finally
        {
            changing = false;
            navigation.IsEnabled = true;
            refresh.IsEnabled = true;
            pageHost.IsEnabled = true;
        }
        if (succeeded && !windowToken.IsCancellationRequested) await RefreshPageAsync();
        return succeeded;
    }

    private async Task<bool> ConfirmAsync(string title, string message)
    {
        dialogs++;
        try { return await Dialog.ConfirmAsync(this, title, message, Localized.Get("确认"), Localized.Get("取消"), true); }
        finally { dialogs--; }
    }

    private async Task ShowDialogAsync(string title, Control content)
    {
        dialogs++;
        try { await Dialog.ShowAsync(this, title, content); }
        finally { dialogs--; }
    }
}
