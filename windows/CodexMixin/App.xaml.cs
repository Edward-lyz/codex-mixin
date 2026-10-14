using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;

namespace CodexMixin;

// WinUI requires a public application entry type; all application logic stays internal.
public partial class App : Application
{
    private MainWindow? window;

    public App() => InitializeComponent();

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        var instance = AppInstance.FindOrRegisterForKey("CodexMixin.Windows");
        if (!instance.IsCurrent)
        {
            await instance.RedirectActivationToAsync(AppInstance.GetCurrent().GetActivatedEventArgs());
            Exit();
            return;
        }
        window = new MainWindow();
        instance.Activated += (_, _) => window.DispatcherQueue.TryEnqueue(window.ShowMain);
        window.Activate();
    }
}
