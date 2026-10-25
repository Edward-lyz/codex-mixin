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
    private TrayWindow? tray;
    private EventWaitHandle? trayReady;
    private bool exiting;
    private ElementTheme theme = ElementTheme.Default;

    internal CliClient Client => cli ??= CliClient.Locate();
    internal Operations Operations => operations ??= new Operations(Client);
    internal ElementTheme Theme => theme;
    internal bool HasPendingChanges => windows.Values.Any(target => target.IsChanging) || tray?.IsChanging == true;
    internal void SetTheme(ElementTheme value)
    {
        theme = value;
        foreach (var target in windows.Values) target.SetTheme(value);
        tray?.SetTheme(value);
    }

    internal void ExitAll()
    {
        if (exiting) return;
        exiting = true;
        trayReady?.Dispose();
        tray?.Shutdown();
        foreach (var target in windows.Values.ToArray()) target.Shutdown();
        Exit();
    }

    internal void OpenPage(string page, bool show = true)
    {
        if (exiting) return;
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
        try { OpenPage("providers", show: !Environment.GetCommandLineArgs().Contains("--background")); }
        catch (Exception error) { Console.Error.WriteLine($"Windows startup failed: {error}"); throw; }
        if (Environment.GetCommandLineArgs().Contains("--capture-ui"))
        {
            trayReady = new EventWaitHandle(false, EventResetMode.ManualReset, $"Local\\CodexMixin.TrayReady.{Environment.ProcessId}");
            trayReady.Set();
        }
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
