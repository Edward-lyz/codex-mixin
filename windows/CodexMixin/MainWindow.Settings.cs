using iNKORE.UI.WPF.Modern;
using System.Diagnostics;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using Microsoft.Win32;

namespace CodexMixin;

public sealed partial class MainWindow
{
    private Task SettingsAsync(CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
        var app = (App)Application.Current;
        var pageToken = pageRefresh!.Token;
        // Recovery and update controls are usable before any CLI query completes.
        var diagnostics = Row();
        var guiLogs = Button(Localized.Get("打开界面日志"), async () =>
        {
            Directory.CreateDirectory(Diagnostics.DirectoryPath);
            await OpenPathAsync(Diagnostics.DirectoryPath);
        });
        AutomationProperties.SetAutomationId(guiLogs, "open-gui-logs");
        diagnostics.Children.Add(guiLogs);
        diagnostics.Children.Add(Button(Localized.Get("打开网关日志"), async () =>
        {
            await ConnectCliAsync(lifetime.Token);
            await OpenPathAsync(Path.GetDirectoryName(Json.Text(Json.Field(metadata, "paths"), "gateway_log"))!);
        }));
        diagnostics.Children.Add(Button(Localized.Get("打开配置目录"), async () =>
        {
            await ConnectCliAsync(lifetime.Token);
            await OpenPathAsync(Json.Text(Json.Field(metadata, "paths"), "state"));
        }));
        diagnostics.Children.Add(Button(Localized.Get("运行诊断"), () => ChangeAsync(Localized.Get("诊断"), ["doctor", "--json"])));
        var about = new iNKORE.UI.WPF.Modern.Controls.SettingsCard
        {
            Header = Localized.Get("关于与诊断"),
            Description = $"Codex Mixin · GUI {AppUpdates.GuiVersion} · iNKORE.UI.WPF.Modern",
            HeaderIcon = new FontIcon { Glyph = "\uE946" },
            Content = diagnostics,
            ContentAlignment = iNKORE.UI.WPF.Modern.Controls.ContentAlignment.Vertical
        };
        var updateControls = UpdateControls(app.Updates, pageToken);
        var theme = Choice(Localized.Get("外观"), [Localized.Get("跟随系统"), Localized.Get("浅色"), Localized.Get("深色")], ThemeManager.Current.ApplicationTheme == ApplicationTheme.Light ? Localized.Get("浅色") : ThemeManager.Current.ApplicationTheme == ApplicationTheme.Dark ? Localized.Get("深色") : Localized.Get("跟随系统"));
        AutomationProperties.SetAutomationId(theme, "theme-picker");
        theme.SelectionChanged += (_, _) => app.SetTheme(theme.SelectedIndex switch { 1 => ApplicationTheme.Light, 2 => ApplicationTheme.Dark, _ => null });
        var startup = new ToggleSwitch { IsOn = NativeShell.StartupEnabled(), IsEnabled = !NativeShell.StartupBlocked(), OnContent = "", OffContent = "" };
        AutomationProperties.SetName(startup, Localized.Get("登录 Windows 时启动界面"));
        var restoringStartup = false;
        startup.Toggled += (_, _) =>
        {
            if (restoringStartup) return;
            try { NativeShell.SetStartup(startup.IsOn); }
            catch (Exception error) { restoringStartup = true; startup.IsOn = NativeShell.StartupEnabled(); restoringStartup = false; ShowNotice(Localized.Get("启动设置失败"), error.Message, InfoBarSeverity.Error); }
        };
        PageBody.Children.Add(new iNKORE.UI.WPF.Modern.Controls.SettingsCard
        {
            Header = Localized.Get("外观"),
            HeaderIcon = new FontIcon { Glyph = "\uE790" },
            Content = theme
        });
        PageBody.Children.Add(new iNKORE.UI.WPF.Modern.Controls.SettingsCard
        {
            Header = Localized.Get("登录 Windows 时启动界面"),
            Description = NativeShell.StartupBlocked() ? Localized.Get("启动项已被 Windows 禁用，请在系统「启动应用」中调整。") : Localized.Get("界面登录启动与网关自启分别管理。"),
            HeaderIcon = new FontIcon { Glyph = "\uE7E8" },
            Content = startup
        });
        PageBody.Children.Add(about);
        PageBody.Children.Add(updateControls);
        var backup = Row();
        backup.Children.Add(Button(Localized.Get("导出配置"), async () =>
        {
            if (!await ConfirmAsync(Localized.Get("导出包含凭据的备份？"), Localized.Get("Base64 不是加密。备份包含供应商密钥，请保存到安全位置。"), Localized.Get("选择保存位置"))) return;
            var picker = new SaveFileDialog { FileName = "codex-mixin-config", DefaultExt = ".b64", Filter = "Codex Mixin backup (*.b64)|*.b64" };
            if (picker.ShowDialog(this) == true) await ChangeAsync(Localized.Get("导出配置"), ["config", "export", picker.FileName]);
        }));
        backup.Children.Add(Button(Localized.Get("导入配置"), async () =>
        {
            var picker = new OpenFileDialog { Filter = "Codex Mixin backup (*.b64)|*.b64" };
            if (picker.ShowDialog(this) == true && await ConfirmAsync(Localized.Get("替换当前配置？"), Localized.Get("导入会替换现有供应商与密钥，并应用到已接入的客户端。"), Localized.Get("导入")))
                await ChangeAsync(Localized.Get("导入配置"), ["config", "import", picker.FileName], apply: true);
        }));
        PageBody.Children.Add(new iNKORE.UI.WPF.Modern.Controls.SettingsCard
        {
            Header = Localized.Get("配置备份"),
            HeaderIcon = new FontIcon { Glyph = "\uE8B7" },
            Content = backup
        });

        var service = new ContentControl();
        var network = new ContentControl();
        PageBody.Children.Add(service);
        PageBody.Children.Add(network);
        Task RefreshService() => LoadSettingAsync(service, Localized.Get("网关自启"), async cancellation =>
        {
            var autostart = await app.Client.QueryAsync(["service", "autostart", "status", "--json"], cancellation);
            var buttons = Row();
            buttons.Children.Add(Button(Localized.Get("启用网关自启"), () => ChangeAsync(Localized.Get("启用网关自启"), ["service", "autostart", "enable", "--json"])));
            buttons.Children.Add(Button(Localized.Get("关闭网关自启"), () => ChangeAsync(Localized.Get("关闭网关自启"), ["service", "autostart", "disable", "--json"])));
            return new iNKORE.UI.WPF.Modern.Controls.SettingsCard
            {
                Header = Localized.Get("网关自启"),
                Description = Json.Flag(autostart, "enabled") ? Localized.Get("已启用") : Localized.Get("已关闭"),
                HeaderIcon = new FontIcon { Glyph = "\uE7E8" },
                Content = buttons
            };
        }, pageToken);
        Task RefreshNetwork() => LoadSettingAsync(network, Localized.Get("官方连接 · ECH"), EchCardAsync, pageToken);
        refreshSettings = () => Task.WhenAll(RefreshService(), RefreshNetwork());
        _ = refreshSettings();
        return Task.CompletedTask;
    }

    private async Task<UIElement> EchCardAsync(CancellationToken cancellation)
    {
        var ech = await ((App)Application.Current).Client.QueryAsync(["ech", "status", "--json"], cancellation);
        var enabled = Json.Flag(ech, "enabled");
        var buttons = Row();
        buttons.Children.Add(Button(Localized.Get("检测 ECH"), () => ChangeAsync(Localized.Get("检测 ECH"), ["ech", "test", "--json"])));
        buttons.Children.Add(Button(Localized.Get("启用 ECH"), () => ChangeAsync(Localized.Get("启用 ECH"), ["ech", "enable"])));
        buttons.Children.Add(Button(Localized.Get("关闭 ECH"), () => ChangeAsync(Localized.Get("关闭 ECH"), ["ech", "disable"])));
        return new iNKORE.UI.WPF.Modern.Controls.SettingsCard
        {
            Header = Localized.Get("官方连接 · ECH"),
            Description = (enabled ? Localized.Get("已启用") : Localized.Get("已关闭")) + " · " + (Json.OptionalText(ech, "fallback_reason") ?? Json.Text(ech, "doh_url")),
            HeaderIcon = new FontIcon { Glyph = "\uE72E" },
            Content = buttons,
            ContentAlignment = iNKORE.UI.WPF.Modern.Controls.ContentAlignment.Vertical
        };
    }

    private async Task LoadSettingAsync(ContentControl host, string title,
        Func<CancellationToken, Task<UIElement>> read, CancellationToken token)
    {
        host.Content ??= Card(title, Localized.Get("正在读取…"));
        try
        {
            var content = await read(token);
            token.ThrowIfCancellationRequested();
            host.Content = content;
        }
        catch (Exception error)
        {
            if (token.IsCancellationRequested) return;
            host.Content = Card(title, Localized.Format("读取失败：{0}", error.Message), Button(Localized.Get("重试"), () => LoadSettingAsync(host, title, read, token)));
        }
    }

    private UIElement UpdateControls(AppUpdates updates, CancellationToken token)
    {
        var status = Text(updates.Status);
        AutomationProperties.SetAutomationId(status, "app-update-status");
        var check = Button(Localized.Get("立即更新"), () => updates.CheckAsync());
        AutomationProperties.SetAutomationId(check, "check-app-updates");
        var releases = Button(Localized.Get("官方发布页"), () =>
        {
            if (Process.Start(new ProcessStartInfo(AppRelease.ReleasesUrl) { UseShellExecute = true }) == null) throw new IOException(Localized.Get("无法打开发布页。"));
            return Task.CompletedTask;
        });
        var automatic = new ToggleSwitch { IsOn = updates.Automatic, OnContent = "", OffContent = "" };
        AutomationProperties.SetName(automatic, Localized.Get("自动更新（每小时检查；网关空闲 30 分钟后安装）"));
        var restoringAutomatic = false;
        automatic.Toggled += (_, _) =>
        {
            if (restoringAutomatic) return;
            try { updates.SetAutomatic(automatic.IsOn); }
            catch (Exception error) { restoringAutomatic = true; automatic.IsOn = updates.Automatic; restoringAutomatic = false; ShowNotice(Localized.Get("更新设置失败"), error.Message, InfoBarSeverity.Error); }
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
        actions.Children.Add(check); actions.Children.Add(releases);
        var body = new Stack { Spacing = 12 };
        body.Children.Add(status); body.Children.Add(actions);
        body.Children.Add(Text(Localized.Get("应用会自动下载、验证签名、安装并重新打开；网关原先运行时也会恢复。安装版可能出现系统 UAC 授权提示。请求进行中或草稿未保存时暂缓安装。")));
        var cards = new Stack { Spacing = 12 };
        cards.Children.Add(new iNKORE.UI.WPF.Modern.Controls.SettingsCard
        {
            Header = Localized.Get("应用更新"),
            Description = Localized.Format("GUI {0} · 完整应用包包含 GUI、CLI 和运行时", AppUpdates.GuiVersion),
            HeaderIcon = new FontIcon { Glyph = "\uE895" },
            Content = body,
            ContentAlignment = iNKORE.UI.WPF.Modern.Controls.ContentAlignment.Vertical
        });
        cards.Children.Add(new iNKORE.UI.WPF.Modern.Controls.SettingsCard
        {
            Header = Localized.Get("自动更新（每小时检查；网关空闲 30 分钟后安装）"),
            Content = automatic
        });
        return cards;
    }
}
