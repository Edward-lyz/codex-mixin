using Avalonia;
using Avalonia.Controls;
using Avalonia.Layout;
using TKCode.Arc;
#if WINDOWS
using Microsoft.Win32;
#endif

namespace CodexMixin;

internal sealed partial class MainWindow
{
    private Control BuildApplicationPreferences()
    {
        var body = new StackPanel { Spacing = 12 };
        var startup = new Switch { Content = Localized.Get("登录时启动应用") };
#if WINDOWS
        using (var run = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run"))
            startup.IsChecked = run?.GetValue("CodexMixin") is string;
        var restoringStartup = false;
        startup.IsCheckedChanged += async (_, _) =>
        {
            if (restoringStartup) return;
            try
            {
                using var approved = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run");
                if (startup.IsChecked == true && approved?.GetValue("CodexMixin") is byte[] { Length: > 0 } value && value[0] is 3 or 7)
                    throw new InvalidOperationException(Localized.Get("启动项已被 Windows 禁用，请在系统「启动应用」中启用。"));
                using var run = Registry.CurrentUser.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run", true);
                if (startup.IsChecked == true)
                    run.SetValue("CodexMixin", $"\"{Environment.ProcessPath}\" --background");
                else run.DeleteValue("CodexMixin", false);
            }
            catch (Exception error) when (error is IOException or UnauthorizedAccessException or InvalidOperationException)
            {
                using var run = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run");
                restoringStartup = true;
                startup.IsChecked = run?.GetValue("CodexMixin") is string;
                restoringStartup = false;
                await ShowDialogAsync(Localized.Get("启动项设置失败"), new TextBlock { Text = error.Message });
            }
        };
        body.Children.Add(startup);
        var lightweight = new TKCode.Arc.Button { Content = Localized.Get("进入轻量模式"), Variant = ButtonVariant.Secondary, HorizontalAlignment = HorizontalAlignment.Left };
        lightweight.Click += async (_, _) =>
        {
            if (!await ConfirmAsync(Localized.Get("进入轻量模式"), Localized.Get("关闭界面并保留后台网关；手动打开应用可恢复界面。"))) return;
            if (!await RunActionAsync(["service", "ensure", "--json"])) return;
            if (!await RunActionAsync(["service", "autostart", "enable", "--json"])) return;
            try
            {
                using var settings = Registry.CurrentUser.CreateSubKey(@"Software\CodexMixin", true);
                settings.SetValue("LightweightMode", 1);
                await ((App)Application.Current!).ExitAllAsync();
            }
            catch (Exception error) when (error is IOException or UnauthorizedAccessException)
            { await ShowDialogAsync(Localized.Get("操作失败"), new TextBlock { Text = error.Message }); }
        };
        body.Children.Add(lightweight);
#else
        if (OperatingSystem.IsLinux())
        {
            var directory = Path.Combine(Environment.GetEnvironmentVariable("XDG_CONFIG_HOME")
                ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".config"), "autostart");
            var path = Path.Combine(directory, "codex-mixin.desktop");
            startup.IsChecked = File.Exists(path);
            startup.IsCheckedChanged += async (_, _) =>
            {
                try
                {
                    if (startup.IsChecked != true) { File.Delete(path); return; }
                    var executable = Environment.ProcessPath ?? throw new IOException(Localized.Get("无法定位应用程序。"));
                    executable = executable.Replace("\\", "\\\\", StringComparison.Ordinal).Replace("\"", "\\\"", StringComparison.Ordinal)
                        .Replace("`", "\\`", StringComparison.Ordinal).Replace("$", "\\$", StringComparison.Ordinal);
                    Directory.CreateDirectory(directory);
                    await File.WriteAllTextAsync(path, $"[Desktop Entry]\nType=Application\nName=Codex Mixin\nExec=\"{executable}\" --background\nTerminal=false\n", windowToken);
                }
                catch (Exception error) when (error is IOException or UnauthorizedAccessException or OperationCanceledException)
                { if (!windowToken.IsCancellationRequested) await ShowDialogAsync(Localized.Get("启动项设置失败"), new TextBlock { Text = error.Message }); }
            };
            body.Children.Add(startup);
        }
        else
        {
            var login = new TKCode.Arc.Button { Content = Localized.UseEnglish ? "Open login items" : "打开系统登录项", Variant = ButtonVariant.Secondary };
            login.Click += async (_, _) => await Launcher.LaunchUriAsync(new Uri("x-apple.systempreferences:com.apple.LoginItems-Settings.extension"));
            body.Children.Add(login);
        }
#endif
        body.Children.Add(new TextBlock
        {
            Text = Localized.UseEnglish ? "App startup controls the interface. Gateway autostart is managed separately below." : "应用登录启动控制界面；网关自启在下方单独管理。",
            TextWrapping = Avalonia.Media.TextWrapping.Wrap
        });
        return new ExpandableCard { Header = Localized.Get("应用"), Content = body };
    }
}
