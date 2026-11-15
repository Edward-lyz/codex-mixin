using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Platform;
using Avalonia.Media;
using Avalonia.Threading;
#if WINDOWS
using Microsoft.Win32;
#endif

namespace CodexMixin;

// Framework entry point; all business actions remain in the Rust CLI.
public sealed class App : Application
{
    private TrayIcon? tray;
    private bool exiting;
    private readonly CancellationTokenSource lifetime = new();
    private Task listener = Task.CompletedTask;
    internal bool HasVisibleWindows => (ApplicationLifetime as IClassicDesktopStyleApplicationLifetime)?.MainWindow?.IsVisible == true;
    internal bool HasDraftChanges => ((ApplicationLifetime as IClassicDesktopStyleApplicationLifetime)?.MainWindow as MainWindow)?.HasDraftChanges == true;
    internal bool HasPendingChanges => ((ApplicationLifetime as IClassicDesktopStyleApplicationLifetime)?.MainWindow as MainWindow)?.HasPendingChanges == true;
#if WINDOWS
    private AppUpdates? updates;
    private readonly DispatcherTimer updateTimer = new() { Interval = TimeSpan.FromHours(1) };
    internal AppUpdates Updates => updates ??= new AppUpdates(CliClient.Locate);
#endif

    internal void SetUpdateInputBlocked(bool blocked)
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop && desktop.MainWindow is { } window)
            window.IsEnabled = !blocked;
    }

    internal async Task ExitAllAsync()
    {
        if (exiting) return;
        if (ApplicationLifetime is not IClassicDesktopStyleApplicationLifetime desktop) return;
        if (desktop.MainWindow is MainWindow window && !await window.ConfirmExitAsync()) return;
        exiting = true;
        lifetime.Cancel();
        await listener;
#if WINDOWS
        if (updates != null) await updates.StopAsync();
        updateTimer.Stop();
#endif
        desktop.Shutdown();
    }

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            Name = "Codex Mixin";
            try
            {
#if WINDOWS
                using (var settings = Registry.CurrentUser.CreateSubKey(@"Software\CodexMixin", true))
                {
                    if (desktop.Args?.Contains("--background") == true && settings.GetValue("LightweightMode") is int enabled && enabled != 0)
                    {
                        Dispatcher.UIThread.Post(() => desktop.Shutdown());
                        base.OnFrameworkInitializationCompleted();
                        return;
                    }
                    if (desktop.Args?.Contains("--background") != true) settings.DeleteValue("LightweightMode", false);
                }
#endif
                var window = new MainWindow();
                desktop.MainWindow = window;
                var hosted = desktop.Args?.Contains("--hosted") == true;
                desktop.ShutdownMode = hosted ? ShutdownMode.OnLastWindowClose : ShutdownMode.OnExplicitShutdown;
                listener = Program.ListenAsync(window, lifetime.Token);
                if (!hosted)
                {
                var menu = new NativeMenu();
                foreach (var (key, label) in new[]
                {
                    ("providers", "打开控制台…"), ("agents", "Agent 接入"), ("routing", "模型路由"),
                    ("context", "上下文引擎"), ("usage", "用量统计"), ("settings", "设置与诊断")
                })
                {
                    var item = new NativeMenuItem(Localized.Get(label));
                    item.Click += (_, _) => window.OpenPage(key);
                    menu.Items.Add(item);
                }
                menu.Items.Add(new NativeMenuItemSeparator());
                foreach (var (action, label) in new[] { ("start", "启动网关"), ("stop", "停止网关"), ("restart", "重启网关") })
                {
                    var item = new NativeMenuItem(Localized.Get(label));
                    item.Click += async (_, _) => await window.RunGatewayActionAsync(action);
                    menu.Items.Add(item);
                }
                menu.Items.Add(new NativeMenuItemSeparator());
                var quit = new NativeMenuItem(Localized.Get("退出"));
                quit.Click += async (_, _) => await ExitAllAsync();
                menu.Items.Add(quit);
                using var iconStream = AssetLoader.Open(new Uri("avares://codex_mixin_ui/Assets/logo.png"));
                var icon = new WindowIcon(iconStream);
                window.Icon = icon;
                tray = new TrayIcon { Icon = icon, ToolTipText = "Codex Mixin", Menu = menu, IsVisible = true };
                TrayIcon.SetIcons(this, new TrayIcons { tray });
                tray.Clicked += (_, _) => window.OpenPage("providers");
                }
                window.Closing += async (_, args) =>
                {
                    if (exiting) return;
                    if (!hosted && tray != null) { args.Cancel = true; window.Hide(); return; }
                    if (window.HasDraftChanges || window.HasPendingChanges)
                    { args.Cancel = true; await ExitAllAsync(); }
                };
                desktop.Exit += (_, _) => { exiting = true; lifetime.Cancel(); tray?.Dispose(); tray = null; };
                if (desktop.Args?.Contains("--background") == true)
                    window.Opened += (_, _) => window.Hide();
#if WINDOWS
                Updates.LoadPreferences();
                updateTimer.Tick += async (_, _) => await Updates.CheckAsync(automatic: true);
                updateTimer.Start();
                window.Opened += async (_, _) => await Updates.CheckAsync(automatic: true);
#endif
                var requested = desktop.Args?.FirstOrDefault(arg => arg.StartsWith("--page=", StringComparison.Ordinal));
                if (requested != null) window.OpenPage(requested[7..]);
            }
            catch (Exception error) when (error is IOException or InvalidOperationException)
            {
                lifetime.Cancel();
                tray?.Dispose();
                tray = null;
                desktop.ShutdownMode = ShutdownMode.OnLastWindowClose;
                desktop.MainWindow = new Window
                {
                    Title = "Codex Mixin", Width = 560, Height = 240,
                    Content = new TextBlock { Text = error.Message, TextWrapping = TextWrapping.Wrap, Margin = new Thickness(24) }
                };
            }
        }
        base.OnFrameworkInitializationCompleted();
    }
}
