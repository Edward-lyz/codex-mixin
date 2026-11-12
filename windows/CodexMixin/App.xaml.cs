using Microsoft.Win32;
using System.IO.Pipes;
using System.Text.RegularExpressions;
using System.Windows;
using System.Windows.Threading;

namespace CodexMixin;

// The XAML compiler requires a public application type; all application logic stays internal.
public partial class App : Application
{
    private const string ConsoleWindowKey = "console";
    // Per-user, per-session names: the mutex marks the primary instance, the pipe carries later launches' arguments.
    private static readonly string InstanceKey = $"CodexMixin.Windows.{Environment.UserName}.{System.Diagnostics.Process.GetCurrentProcess().SessionId}";
    private Mutex? instanceMutex;
    private readonly Dictionary<string, MainWindow> windows = [];
    private CliClient? cli;
    private Operations? operations;
    private AppUpdates? updates;
    private readonly DispatcherTimer updateTimer = new() { Interval = TimeSpan.FromHours(1) };
    private TrayWindow? tray;
    private bool exiting;
    private string? notifiedUpdate;

    internal CliClient Client => cli ??= CliClient.Locate();
    internal Operations Operations => operations ??= new Operations(Client);
    internal AppUpdates Updates => updates ??= new AppUpdates(() => Client);
    internal bool IsUpdating => updates?.Installing == true;
    internal bool HasVisibleWindows => windows.Values.Any(target => target.IsVisible);
    internal bool HasDraftChanges => windows.Values.Any(target => target.HasDraftChanges);
    internal bool HasPendingChanges => windows.Values.Any(target => target.IsChanging) || tray?.IsChanging == true;
    // Fluent resources follow Application.ThemeMode in every window.
    internal void SetTheme(ThemeMode value) => ThemeMode = value;

    internal void SetUpdateInputBlocked(bool blocked)
    {
        foreach (var target in windows.Values) target.SetUpdateInputBlocked(blocked);
        tray?.SetUpdateInputBlocked(blocked);
    }

    internal async Task<bool> ConfirmDraftsAsync()
    {
        foreach (var target in windows.Values.Where(target => target.HasDraftChanges).ToArray())
            if (!await target.ConfirmDiscardAsync(Localized.Get("退出应用"), discard: false)) return false;
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
        Shutdown();
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
        // Fusion became one kind of model route; old "fusion" links open the routing page.
        if (page == "fusion") page = "routing";
        if (page is not ("providers" or "agents" or "routing" or "settings" or "usage" or "context"))
            throw new ArgumentException($"Unknown Windows page: {page}");
        tray?.HidePanel();
        // Models, agents, usage, context, and routing share one console window;
        // settings keeps its own window.
        var key = CodexMixin.MainWindow.IsConsolePage(page) ? ConsoleWindowKey : page;
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
        DispatcherUnhandledException += (_, args) => WriteCrash(args.Exception);
        AppDomain.CurrentDomain.UnhandledException += (_, args) => WriteCrash(args.ExceptionObject);
        InitializeComponent();
    }

    private static void WriteCrash(object error)
    {
        var directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMixin", "logs");
        Directory.CreateDirectory(directory);
        File.WriteAllText(Path.Combine(directory, "native-error.txt"), error.ToString());
    }

    protected override void OnStartup(StartupEventArgs args)
    {
        base.OnStartup(args);
        ThemeMode = ThemeMode.System;
        var background = args.Args.Contains("--background");
        if (background && LightweightModeSet()) { Shutdown(); return; }
        SetLightweightMode(false);
        instanceMutex = new Mutex(true, InstanceKey, out var primary);
        if (!primary)
        {
            instanceMutex.Dispose();
            instanceMutex = null;
            ForwardToPrimary(string.Join(' ', args.Args));
            Shutdown();
            return;
        }
        _ = ListenForActivationsAsync();
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

    protected override void OnExit(ExitEventArgs args)
    {
        instanceMutex?.ReleaseMutex();
        instanceMutex?.Dispose();
        base.OnExit(args);
    }

    private static void ForwardToPrimary(string arguments)
    {
        // The primary may still be starting its listener; a short timeout keeps a stuck primary from hanging this launch.
        try
        {
            using var pipe = new NamedPipeClientStream(".", InstanceKey, PipeDirection.Out, PipeOptions.CurrentUserOnly);
            pipe.Connect(5000);
            using var writer = new StreamWriter(pipe);
            writer.WriteLine(arguments);
        }
        catch (Exception error) when (error is TimeoutException or IOException)
        {
            Console.Error.WriteLine($"Could not reach the running Codex Mixin: {error.Message}");
        }
    }

    private async Task ListenForActivationsAsync()
    {
        while (!exiting)
        {
            try
            {
                await using var pipe = new NamedPipeServerStream(InstanceKey, PipeDirection.In, 1,
                    PipeTransmissionMode.Byte, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
                await pipe.WaitForConnectionAsync();
                using var reader = new StreamReader(pipe);
                var arguments = await reader.ReadLineAsync() ?? "";
                var match = Regex.Match(arguments, @"--page=(providers|agents|routing|fusion|settings|overview|usage|context)\b");
                OpenPage(match.Success ? match.Groups[1].Value : "providers");
            }
            catch (Exception error) when (error is IOException or ArgumentException)
            {
                Console.Error.WriteLine($"Windows activation failed: {error.Message}");
            }
        }
    }
}
