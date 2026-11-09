using Microsoft.Win32;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;
using System.Runtime.InteropServices;

namespace CodexMixin;

// WinUI requires a public application entry type; all application logic stays internal.
public partial class App : Application
{
    private const string ConsoleWindowKey = "console";
    private const string InstanceKey = "CodexMixin.Windows";
    private const int CatastrophicFailure = unchecked((int)0x8000FFFF);
    private const int InstanceRegistrationAttempts = 5;
    private readonly Dictionary<string, MainWindow> windows = [];
    private CliClient? cli;
    private Operations? operations;
    private AppUpdates? updates;
    private readonly DispatcherTimer updateTimer = new() { Interval = TimeSpan.FromHours(1) };
    private TrayWindow? tray;
    private bool exiting;
    private string? notifiedUpdate;
    private ElementTheme theme = ElementTheme.Default;

    internal CliClient Client => cli ??= CliClient.Locate();
    internal Operations Operations => operations ??= new Operations(Client);
    internal AppUpdates Updates => updates ??= new AppUpdates(() => Client);
    internal ElementTheme Theme => theme;
    internal bool IsUpdating => updates?.Installing == true;
    internal bool HasVisibleWindows => windows.Values.Any(target => target.AppWindow.IsVisible);
    internal bool HasDraftChanges => windows.Values.Any(target => target.HasDraftChanges);
    internal bool HasPendingChanges => windows.Values.Any(target => target.IsChanging) || tray?.IsChanging == true;
    internal void SetTheme(ElementTheme value)
    {
        theme = value;
        foreach (var target in windows.Values) target.SetTheme(value);
        tray?.SetTheme(value);
    }

    internal void SetUpdateInputBlocked(bool blocked)
    {
        foreach (var target in windows.Values) target.SetUpdateInputBlocked(blocked);
        tray?.SetUpdateInputBlocked(blocked);
    }

    internal async Task<bool> ConfirmDraftsAsync()
    {
        foreach (var target in windows.Values.Where(target => target.HasDraftChanges).ToArray())
            if (!await target.ConfirmDiscardAsync("退出应用", discard: false)) return false;
        return true;
    }

    internal async Task ExitAllAsync()
    {
        if (exiting) return;
        exiting = true;
        updateTimer.Stop();
        if (updates != null) await updates.StopAsync();
        tray?.Shutdown();
        foreach (var target in windows.Values.ToArray()) target.Shutdown();
        Exit();
    }

    // Lightweight mode: only the CLI gateway stays resident. The login launch
    // of this UI exits at once until the user opens the app by hand.
    private const string ShellSettingsKey = @"Software\CodexMixin";
    private const string LightweightValue = "LightweightMode";

    private static bool LightweightModeSet()
    {
        using var settings = Registry.CurrentUser.OpenSubKey(ShellSettingsKey);
        return settings?.GetValue(LightweightValue) is int enabled && enabled != 0;
    }

    private static void SetLightweightMode(bool enabled)
    {
        using var settings = Registry.CurrentUser.CreateSubKey(ShellSettingsKey, true);
        if (enabled) settings.SetValue(LightweightValue, 1);
        else settings.DeleteValue(LightweightValue, false);
    }

    /// Keeps the gateway running at login, then closes every window and the tray.
    internal async Task EnterLightweightModeAsync()
    {
        await Operations.ChangeAsync(["service", "autostart", "enable", "--json"], CancellationToken.None);
        SetLightweightMode(true);
        await ExitAllAsync();
    }

    /// Stops the gateway as well as the UI.
    internal async Task QuitCompletelyAsync()
    {
        await Operations.ChangeAsync(
            ["service", "stop", "--managed", "--allow-codex-disconnect", "--json"], CancellationToken.None);
        await ExitAllAsync();
    }

    internal void OpenPage(string page, bool show = true)
    {
        if (exiting || IsUpdating) return;
        if (page == "updates")
        {
            OpenPage("settings");
            _ = Updates.CheckAsync();
            return;
        }
        if (page == "overview")
        {
            tray ??= new TrayWindow();
            tray.ShowPanel();
            return;
        }
        // "console" reopens the console on the page it last showed.
        if (page == ConsoleWindowKey)
        {
            tray?.HidePanel();
            if (windows.TryGetValue(ConsoleWindowKey, out var console)) { console.ShowMain(); return; }
            page = "providers";
        }
        if (page is not ("providers" or "agents" or "fusion" or "settings" or "usage" or "context"))
            throw new ArgumentException($"Unknown Windows page: {page}");
        tray?.HidePanel();
        // Models, agents, context, and usage share one console window; Fusion and
        // settings keep their own windows.
        var key = MainWindow.IsConsolePage(page) ? ConsoleWindowKey : page;
        if (!windows.TryGetValue(key, out var target))
        {
            target = new MainWindow(page);
            windows[key] = target;
            target.Closed += (_, _) => windows.Remove(key);
            if (show) target.ShowMain();
            return;
        }
        target.ShowPage(page);
    }

    public App()
    {
        UnhandledException += (_, args) =>
        {
            var directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMixin", "logs");
            Directory.CreateDirectory(directory);
            File.WriteAllText(Path.Combine(directory, "native-error.txt"), args.Exception.ToString());
        };
        InitializeComponent();
    }

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        var background = Environment.GetCommandLineArgs().Contains("--background");
        if (background && LightweightModeSet()) { Exit(); return; }
        SetLightweightMode(false);
        var instance = await FindMainInstanceAsync();
        if (!instance.IsCurrent)
        {
            await instance.RedirectActivationToAsync(AppInstance.GetCurrent().GetActivatedEventArgs());
            Exit();
            return;
        }
        var dispatcher = Microsoft.UI.Dispatching.DispatcherQueue.GetForCurrentThread();
        instance.Activated += (_, activation) => dispatcher.TryEnqueue(() =>
        {
            var arguments = activation.Data switch
            {
                Windows.ApplicationModel.Activation.ICommandLineActivatedEventArgs command => command.Operation.Arguments,
                Windows.ApplicationModel.Activation.ILaunchActivatedEventArgs launch => launch.Arguments,
                _ => ""
            };
            var match = System.Text.RegularExpressions.Regex.Match(arguments, @"--page=(providers|agents|fusion|settings|overview|usage|context)\b");
            OpenPage(match.Success ? match.Groups[1].Value : "providers");
        });
        try { OpenPage("providers", show: !background); }
        catch (Exception error) { Console.Error.WriteLine($"Windows startup failed: {error}"); throw; }
        // Prime only the compact panel; do not load the hidden console/model editor.
        tray = new TrayWindow();
        _ = tray.PreloadAsync();
        Updates.LoadPreferences();
        if (Updates.LastInstallFailed) OpenPage("settings");
        Updates.Changed += () =>
        {
            if (Updates.Release is { IsNewer: true } release && notifiedUpdate != release.Version
                && windows.TryGetValue(ConsoleWindowKey, out var console) && console.NotifyUpdate(release.Version))
                notifiedUpdate = release.Version;
        };
        updateTimer.Tick += async (_, _) => await Updates.CheckAsync(automatic: true);
        updateTimer.Start();
        _ = Updates.CheckAsync(automatic: true);
    }

    private static async Task<AppInstance> FindMainInstanceAsync()
    {
        for (var attempt = 0; ; attempt++)
        {
            try
            {
                return AppInstance.FindOrRegisterForKey(InstanceKey);
            }
            catch (COMException error) when (
                error.HResult == CatastrophicFailure
                && attempt + 1 < InstanceRegistrationAttempts)
            {
                // Unpackaged AppLifecycle registration intermittently returns
                // E_UNEXPECTED while short-lived activation processes exit.
                // Retry only that transient HRESULT for a bounded period.
                Console.Error.WriteLine(
                    $"Retrying Windows instance registration after {error.HResult:X8}");
                await Task.Delay(TimeSpan.FromMilliseconds(100 * (1 << attempt)));
            }
        }
    }
}
