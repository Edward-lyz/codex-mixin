using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;

namespace CodexMixin;

// WinUI requires a public application entry type; all application logic stays internal.
public partial class App : Application
{
    private MainWindow? window;
    private readonly Dictionary<string, MainWindow> windows = [];
    private CliClient? cli;
    private Operations? operations;
    private TrayWindow? tray;
    private ElementTheme theme = ElementTheme.Default;

    internal CliClient Client => cli ??= CliClient.Locate();
    internal Operations Operations => operations ??= new Operations(Client);
    internal ElementTheme Theme => theme;
    internal static void StartupTrace(string stage)
    {
        var directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMixin", "logs");
        Directory.CreateDirectory(directory);
        File.AppendAllText(Path.Combine(directory, "startup-trace.txt"), $"{DateTime.UtcNow:O} {stage}\n");
    }
    internal void SetTheme(ElementTheme value)
    {
        theme = value;
        foreach (var target in windows.Values) target.SetTheme(value);
        tray?.SetTheme(value);
    }

    internal void ExitAll()
    {
        tray?.Shutdown();
        foreach (var target in windows.Values.ToArray()) target.Shutdown();
        Exit();
    }

    internal void OpenPage(string page)
    {
        if (page == "overview")
        {
            tray ??= new TrayWindow();
            tray.ShowPanel();
            return;
        }
        if (page is not ("providers" or "agents" or "fusion" or "settings" or "usage"))
            throw new ArgumentException($"Unknown Windows page: {page}");
        tray?.HidePanel();
        if (!windows.TryGetValue(page, out var target))
        {
            target = new MainWindow(page);
            windows[page] = target;
            target.Closed += (_, _) => windows.Remove(page);
        }
        target.ShowMain();
    }

    public App()
    {
        StartupTrace("application constructor");
        if (Environment.GetCommandLineArgs().Contains("--capture-ui"))
            AppDomain.CurrentDomain.FirstChanceException += (_, args) => StartupTrace($"managed exception: {args.Exception}");
        UnhandledException += (_, args) =>
        {
            var directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMixin", "logs");
            Directory.CreateDirectory(directory);
            File.WriteAllText(Path.Combine(directory, "native-error.txt"), args.Exception.ToString());
        };
        InitializeComponent();
        StartupTrace("application resources initialized");
    }

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        StartupTrace("application launched");
        var instance = AppInstance.FindOrRegisterForKey("CodexMixin.Windows");
        if (!instance.IsCurrent)
        {
            await instance.RedirectActivationToAsync(AppInstance.GetCurrent().GetActivatedEventArgs());
            Exit();
            return;
        }
        try { OpenPage("providers"); }
        catch (Exception error) { Console.Error.WriteLine($"Windows startup failed: {error}"); throw; }
        window = windows["providers"];
        StartupTrace("provider window opened");
        instance.Activated += (_, activation) => window.DispatcherQueue.TryEnqueue(() =>
        {
            var arguments = activation.Data switch
            {
                Windows.ApplicationModel.Activation.ICommandLineActivatedEventArgs command => command.Operation.Arguments,
                Windows.ApplicationModel.Activation.ILaunchActivatedEventArgs launch => launch.Arguments,
                _ => ""
            };
            var match = System.Text.RegularExpressions.Regex.Match(arguments, @"--page=(providers|agents|fusion|settings|overview|usage)\b");
            OpenPage(match.Success ? match.Groups[1].Value : "providers");
        });
    }
}
