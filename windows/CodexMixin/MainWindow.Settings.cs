using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Windows.Storage.Pickers;
using Windows.System;
using WinRT.Interop;

namespace CodexMixin;

public sealed partial class MainWindow
{
    private Task SettingsAsync(CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
        var app = (App)Microsoft.UI.Xaml.Application.Current;
        var pageToken = pageRefresh!.Token;
        // Recovery and update controls are usable before any CLI query completes.
        var diagnostics = Row();
        var guiLogs = Button("打开界面日志", async () =>
        {
            Directory.CreateDirectory(Diagnostics.DirectoryPath);
            await OpenPathAsync(Diagnostics.DirectoryPath);
        });
        AutomationProperties.SetAutomationId(guiLogs, "open-gui-logs");
        diagnostics.Children.Add(guiLogs);
        diagnostics.Children.Add(Button("打开网关日志", async () =>
        {
            await ConnectCliAsync(lifetime.Token);
            await OpenPathAsync(Path.GetDirectoryName(Json.Text(Json.Field(metadata, "paths"), "gateway_log"))!);
        }));
        diagnostics.Children.Add(Button("打开配置目录", async () =>
        {
            await ConnectCliAsync(lifetime.Token);
            await OpenPathAsync(Json.Text(Json.Field(metadata, "paths"), "state"));
        }));
        diagnostics.Children.Add(Button("运行诊断", () => ChangeAsync("诊断", ["doctor", "--json"])));
        diagnostics.Children.Add(Button("重放上报", () => ChangeAsync("重放上报", ["report-replay"])));
        var about = Card("关于与诊断", $"Codex Mixin · GUI {AppUpdates.GuiVersion} · WinUI 3 / Windows App SDK", diagnostics);
        var updateControls = UpdateControls(app.Updates, pageToken);
        var theme = Choice("外观", ["跟随系统", "浅色", "深色"], Root.RequestedTheme switch { ElementTheme.Light => "浅色", ElementTheme.Dark => "深色", _ => "跟随系统" });
        AutomationProperties.SetAutomationId(theme, "theme-picker");
        theme.SelectionChanged += (_, _) => app.SetTheme(theme.SelectedIndex switch { 1 => ElementTheme.Light, 2 => ElementTheme.Dark, _ => ElementTheme.Default });
        var startup = new CheckBox { Content = "登录 Windows 时启动界面", IsChecked = NativeShell.StartupEnabled(), IsEnabled = !NativeShell.StartupBlocked() };
        startup.Click += (_, _) =>
        {
            try { NativeShell.SetStartup(startup.IsChecked == true); }
            catch (Exception error) { startup.IsChecked = NativeShell.StartupEnabled(); ShowNotice("启动设置失败", error.Message, InfoBarSeverity.Error); }
        };
        var appearance = new StackPanel { Spacing = 16 }; appearance.Children.Add(theme); appearance.Children.Add(startup);
        appearance.Children.Add(Text(NativeShell.StartupBlocked() ? "启动项已被 Windows 禁用，请在系统「启动应用」中调整。" : "界面登录启动与网关自启分别管理。"));
        PageBody.Children.Add(Card("外观与启动", "", appearance));
        PageBody.Children.Add(about);
        PageBody.Children.Add(updateControls);
        var backup = Row();
        backup.Children.Add(Button("导出配置", async () =>
        {
            if (!await ConfirmAsync("导出包含凭据的备份？", "Base64 不是加密。备份包含供应商密钥，请保存到安全位置。", "选择保存位置")) return;
            var picker = new FileSavePicker { SuggestedFileName = "codex-mixin-config" };
            picker.FileTypeChoices.Add("Codex Mixin backup", new List<string> { ".b64" });
            InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
            var file = await picker.PickSaveFileAsync();
            if (file != null) await ChangeAsync("导出配置", ["config", "export", file.Path]);
        }));
        backup.Children.Add(Button("导入配置", async () =>
        {
            var picker = new FileOpenPicker(); picker.FileTypeFilter.Add(".b64");
            InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
            var file = await picker.PickSingleFileAsync();
            if (file != null && await ConfirmAsync("替换当前配置？", "导入会替换现有供应商与密钥，并应用到已接入的客户端。", "导入"))
                await ChangeAsync("导入配置", ["config", "import", file.Path], apply: true);
        }));
        PageBody.Children.Add(Card("配置备份", "", backup));

        var service = new ContentControl();
        var network = new ContentControl();
        PageBody.Children.Add(service);
        PageBody.Children.Add(network);
        _ = LoadSettingAsync(service, "网关自启", async cancellation =>
        {
            var autostart = await app.Client.QueryAsync(["service", "autostart", "status", "--json"], cancellation);
            var buttons = Row();
            buttons.Children.Add(Button("启用网关自启", () => ChangeAsync("启用网关自启", ["service", "autostart", "enable", "--json"])));
            buttons.Children.Add(Button("关闭网关自启", () => ChangeAsync("关闭网关自启", ["service", "autostart", "disable", "--json"])));
            return Card("网关自启", Json.Flag(autostart, "enabled") ? "已启用" : "已关闭", buttons);
        }, pageToken);
        _ = LoadSettingAsync(network, "官方连接 · ECH", async cancellation =>
        {
            var ech = await app.Client.QueryAsync(["ech", "status", "--json"], cancellation);
            var buttons = Row();
            buttons.Children.Add(Button("检测 ECH", () => ChangeAsync("检测 ECH", ["ech", "test", "--json"])));
            buttons.Children.Add(Button("启用 ECH", () => ChangeAsync("启用 ECH", ["ech", "enable"])));
            buttons.Children.Add(Button("关闭 ECH", () => ChangeAsync("关闭 ECH", ["ech", "disable"])));
            return Card("官方连接 · ECH", $"{(Json.Flag(ech, "enabled") ? "已启用" : "已关闭")} · {Json.OptionalText(ech, "fallback_reason") ?? "未发生连接回退"}", buttons);
        }, pageToken);
        return Task.CompletedTask;
    }

    private async Task LoadSettingAsync(ContentControl host, string title,
        Func<CancellationToken, Task<UIElement>> read, CancellationToken token)
    {
        host.Content = Card(title, "正在读取…");
        try
        {
            var content = await read(token);
            token.ThrowIfCancellationRequested();
            host.Content = content;
        }
        catch (Exception error)
        {
            if (token.IsCancellationRequested) return;
            host.Content = Card(title, $"读取失败：{error.Message}", Button("重试", () => LoadSettingAsync(host, title, read, token)));
        }
    }

    private UIElement UpdateControls(AppUpdates updates, CancellationToken token)
    {
        var status = Text(updates.Status);
        AutomationProperties.SetAutomationId(status, "app-update-status");
        var check = Button("检查更新", () => updates.CheckAsync());
        AutomationProperties.SetAutomationId(check, "check-app-updates");
        var installer = Button("下载完整安装包", async () =>
        {
            var uri = updates.Release?.Installer ?? new Uri(AppRelease.ReleasesUrl);
            if (!await Launcher.LaunchUriAsync(uri)) throw new IOException("无法打开下载地址。");
        });
        AutomationProperties.SetAutomationId(installer, "download-app-installer");
        var download = Button("打开完整便携包下载", async () =>
        {
            var uri = updates.Release?.Portable ?? new Uri(AppRelease.ReleasesUrl);
            if (!await Launcher.LaunchUriAsync(uri)) throw new IOException("无法打开下载地址。");
        });
        var releases = Button("官方发布页", async () =>
        {
            if (!await Launcher.LaunchUriAsync(new Uri(AppRelease.ReleasesUrl))) throw new IOException("无法打开发布页。");
        });
        var automatic = new CheckBox
        {
            Content = "自动检查更新（每小时）",
            IsChecked = updates.Automatic
        };
        automatic.Click += (_, _) =>
        {
            try { updates.SetAutomatic(automatic.IsChecked == true); }
            catch (Exception error) { automatic.IsChecked = updates.Automatic; ShowNotice("更新设置失败", error.Message, InfoBarSeverity.Error); }
        };
        void RefreshUpdateState()
        {
            if (token.IsCancellationRequested) return;
            status.Text = updates.Status;
            check.IsEnabled = !updates.Busy;
        }
        updates.Changed += RefreshUpdateState;
        token.Register(() => updates.Changed -= RefreshUpdateState);
        RefreshUpdateState();
        var actions = Row();
        actions.Children.Add(check); actions.Children.Add(installer); actions.Children.Add(download); actions.Children.Add(releases);
        var body = new StackPanel { Spacing = 12 };
        body.Children.Add(status); body.Children.Add(automatic); body.Children.Add(actions);
        body.Children.Add(Text("下载由浏览器处理。安装前请完成请求并退出应用；安装器需要管理员权限，会关闭网关，尚无失败回滚。便携版需退出后替换整个目录。"));
        return Card("应用更新", $"GUI {AppUpdates.GuiVersion} · 完整应用包包含 GUI、CLI 和运行时", body);
    }
}
