using System.Diagnostics;
using System.Globalization;
using System.Text.Json;
using Avalonia;
using Avalonia.Automation;
using Avalonia.Controls;
using Avalonia.Controls.Templates;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Platform.Storage;
using TKCode.Arc.DesignTokens;
using Arc = TKCode.Arc;

namespace CodexMixin;

// The settings page: gateway lifecycle and autostart, ECH, quota, gateway access
// keys, diagnostics and logs, and configuration backup. Every operation runs
// through the CLI; this page keeps no gateway or configuration state of its own.
internal sealed partial class MainWindow
{
    private sealed record SettingsKey(string Id, string Name, string Masked, string Created);

    private async Task<Control> BuildSettingsAsync(CancellationToken token)
    {
        var page = new StackPanel { Spacing = 12, MaxWidth = 880, HorizontalAlignment = HorizontalAlignment.Left };
        // One failing section keeps the rest of the page usable.
        async Task Section(string title, Func<Task<Control>> build)
        {
            try { page.Children.Add(await build()); }
            catch (Exception error) when (error is CliFailure or IOException or InvalidOperationException or FormatException)
            {
                var retry = new Arc.Button { Content = Localized.Get("重试"), Variant = Arc.ButtonVariant.Secondary, HorizontalAlignment = HorizontalAlignment.Left };
                retry.Click += async (_, _) => await RefreshPageAsync();
                page.Children.Add(new Arc.ExpandableCard
                {
                    Header = title,
                    Description = Localized.Get("读取失败"),
                    IsExpanded = true,
                    Content = ContextAlert(title, error.Message, retry)
                });
            }
        }
        await Section(Localized.Get("本地网关"), () => SettingsGatewayAsync(token));
        await Section(Localized.Get("官方连接 · ECH"), () => SettingsEchAsync(token));
        await Section(Localized.Get("额度"), () => Task.FromResult(SettingsQuota(token)));
        await Section(Localized.Get("访问密钥"), () => SettingsKeysAsync(token));
        await Section(Localized.Get("诊断与日志"), () => SettingsDiagnosticsAsync(token));
#if WINDOWS
        if (OperatingSystem.IsWindows()) page.Children.Add(SettingsAppUpdates(token));
#endif
        page.Children.Add(SettingsBackup());
        return page;
    }

#if WINDOWS
    // Windows keeps WinSparkle updates; macOS hosted pages leave updating to the AppKit shell.
    [System.Runtime.Versioning.SupportedOSPlatform("windows")]
    private Control SettingsAppUpdates(CancellationToken token)
    {
        var updates = ((App)Application.Current!).Updates;
        var status = new TextBlock { TextWrapping = TextWrapping.Wrap };
        var automatic = new Arc.Switch { Content = Localized.Get("自动更新（每小时检查；网关空闲 30 分钟后安装）"), IsChecked = updates.Automatic };
        var check = new Arc.Button { Content = Localized.Get("立即更新"), Variant = Arc.ButtonVariant.Primary };
        var releases = new Arc.Button { Content = Localized.Get("官方发布页"), Variant = Arc.ButtonVariant.Secondary };
        AutomationProperties.SetAutomationId(status, "app-update-status");
        AutomationProperties.SetAutomationId(automatic, "app-update-automatic");
        AutomationProperties.SetAutomationId(check, "check-app-updates");
        void Refresh()
        {
            status.Text = updates.Status;
            check.IsEnabled = !updates.Busy;
        }
        automatic.IsCheckedChanged += (_, _) =>
        {
            try { updates.SetAutomatic(automatic.IsChecked == true); }
            catch (Exception error) when (error is IOException or UnauthorizedAccessException or System.Security.SecurityException)
            {
                automatic.IsChecked = updates.Automatic;
                status.Text = Localized.Format("更新设置失败：{0}", error.Message);
            }
        };
        check.Click += async (_, _) => await updates.CheckAsync();
        releases.Click += async (_, _) => await SettingsGuardAsync(Localized.Get("官方发布页"), () =>
        {
            var start = new ProcessStartInfo(AppRelease.ReleasesUrl) { UseShellExecute = true };
            using var process = Process.Start(start);
            return Task.CompletedTask;
        });
        void Changed() => Avalonia.Threading.Dispatcher.UIThread.Post(Refresh);
        updates.Changed += Changed;
        token.Register(() => updates.Changed -= Changed);
        Refresh();
        var actions = new WrapPanel { Orientation = Orientation.Horizontal };
        check.Margin = new Thickness(0, 0, 8, 8);
        actions.Children.Add(check);
        actions.Children.Add(releases);
        var body = new StackPanel { Spacing = 16 };
        body.Children.Add(status);
        body.Children.Add(automatic);
        body.Children.Add(actions);
        body.Children.Add(ContextCaption(Localized.Get("应用会自动下载、验证签名、安装并重新打开；网关原先运行时也会恢复。安装版可能出现系统 UAC 授权提示。请求进行中或草稿未保存时暂缓安装。")));
        return new Arc.ExpandableCard
        {
            Header = Localized.Get("应用更新"),
            Description = Localized.Format("GUI {0} · 完整应用包包含 GUI、CLI 和运行时", AppUpdates.GuiVersion),
            IsExpanded = true,
            Content = body
        };
    }
#endif

    private static Arc.Button SettingsButton(string label, Func<Task> work, Arc.ButtonVariant variant = Arc.ButtonVariant.Secondary)
    {
        var button = new Arc.Button { Content = label, Variant = variant };
        button.Click += async (_, _) =>
        {
            button.IsLoading = true;
            try { await work(); }
            finally { button.IsLoading = false; }
        };
        return button;
    }

    // Failures of direct CLI reads become a dialog instead of an unhandled exception.
    private async Task SettingsGuardAsync(string title, Func<Task> work)
    {
        try { await work(); }
        catch (OperationCanceledException) when (windowToken.IsCancellationRequested) { }
        catch (Exception error) when (error is CliFailure or IOException or InvalidOperationException or FormatException or JsonException or OperationCanceledException or System.ComponentModel.Win32Exception)
        {
            var message = new TextBlock { Text = error.Message, TextWrapping = TextWrapping.Wrap, MaxWidth = 480 };
            AutomationProperties.SetAutomationId(message, "settings-error");
            await ShowDialogAsync(title, message);
        }
    }

    private async Task<Control> SettingsGatewayAsync(CancellationToken token)
    {
        var status = await cli.QueryAsync(["service", "status", "--json"], token);
        var state = Json.Gateway(status);
        var running = Json.Flag(status, "configured") && Json.Text(status, "gateway") == "running";
        var endpoint = Json.OptionalText(status, "endpoint");
        var autostart = await cli.QueryAsync(["service", "autostart", "status", "--json"], token);

        var body = new StackPanel { Spacing = 16 };
        var summary = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        summary.Children.Add(new Arc.Badge { Content = state, Tone = running ? Arc.BadgeTone.Success : Arc.BadgeTone.Neutral });
        summary.Children.Add(new TextBlock { Text = endpoint ?? Localized.Get("尚无运行中的接口地址"), VerticalAlignment = VerticalAlignment.Center });
        if (endpoint is not null) summary.Children.Add(new Arc.CopyButton { Value = endpoint, Label = Localized.Get("复制地址"), CopiedLabel = Localized.Get("已复制") });
        body.Children.Add(summary);
        if (Json.OptionalText(status, "error") is { } statusError)
            body.Children.Add(new Arc.Alert { Tone = Arc.AlertTone.Warning, Title = Localized.Get("服务诊断"), Content = new TextBlock { Text = statusError, TextWrapping = TextWrapping.Wrap } });

        var lifecycle = new WrapPanel { Orientation = Orientation.Horizontal };
        foreach (var control in new Control[]
        {
            SettingsLifecycle(Localized.Get("启动网关"), ["service", "start", "--managed", "--json"], Arc.ButtonVariant.Primary),
            SettingsLifecycle(Localized.Get("停止网关"), ["service", "stop", "--managed", "--json"], Arc.ButtonVariant.Secondary),
            SettingsLifecycle(Localized.Get("重启网关"), ["service", "restart", "--managed", "--json"], Arc.ButtonVariant.Secondary)
        })
        {
            control.Margin = new Thickness(0, 0, 8, 8);
            lifecycle.Children.Add(control);
        }
        body.Children.Add(lifecycle);

        var automatic = new Arc.Switch { Content = Localized.Get("网关随系统自启"), IsChecked = Json.Flag(autostart, "enabled") };
        AutomationProperties.SetAutomationId(automatic, "gateway-autostart");
        var restoringAutostart = false;
        automatic.IsCheckedChanged += async (_, _) =>
        {
            if (restoringAutostart) return;
            if (await RunActionAsync(["service", "autostart", automatic.IsChecked == true ? "enable" : "disable", "--json"])) return;
            restoringAutostart = true;
            automatic.IsChecked = Json.Flag(autostart, "enabled");
            restoringAutostart = false;
            await RefreshPageAsync();
        };
        body.Children.Add(automatic);
        body.Children.Add(ContextCaption(Localized.Get("网关自启由 CLI 管理，与应用登录启动分别设置。")));
        return new Arc.ExpandableCard
        {
            Header = Localized.Get("本地网关"),
            Description = endpoint is null ? state : $"{state} · {endpoint}",
            IsExpanded = true,
            Content = body
        };
    }

    private Arc.Button SettingsLifecycle(string label, string[] args, Arc.ButtonVariant variant)
    {
        var button = new Arc.Button { Content = label, Variant = variant };
        button.Click += async (_, _) => await RunActionAsync(args);
        return button;
    }

    private async Task<Control> SettingsEchAsync(CancellationToken token)
    {
        var ech = await cli.QueryAsync(["ech", "status", "--json"], token);
        var enabled = Json.Flag(ech, "enabled");
        var detail = Json.OptionalText(ech, "fallback_reason") ?? Json.Text(ech, "doh_url");
        var toggle = new Arc.Switch { Content = Localized.Get("启用 ECH"), IsChecked = enabled };
        AutomationProperties.SetAutomationId(toggle, "ech-enabled");
        var restoringEch = false;
        toggle.IsCheckedChanged += async (_, _) =>
        {
            if (restoringEch) return;
            if (await RunActionAsync(["ech", toggle.IsChecked == true ? "enable" : "disable"])) return;
            restoringEch = true;
            toggle.IsChecked = enabled;
            restoringEch = false;
            await RefreshPageAsync();
        };
        var test = SettingsButton(Localized.Get("检测 ECH"), () => SettingsGuardAsync(Localized.Get("检测 ECH"), async () =>
        {
            var result = await cli.QueryAsync(["ech", "test", "--json"], windowToken, TimeSpan.FromSeconds(60));
            var probes = new StackPanel { Spacing = 8, MinWidth = 420 };
            foreach (var probe in Json.Rows(Json.Field(result, "probes")))
            {
                var accepted = Json.Flag(probe, "ech_accepted");
                var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
                row.Children.Add(new Arc.Badge { Content = accepted ? Localized.Get("ECH 已接受") : Localized.Get("ECH 未接受"), Tone = accepted ? Arc.BadgeTone.Success : Arc.BadgeTone.Danger });
                row.Children.Add(new TextBlock { Text = Json.Text(probe, "host"), VerticalAlignment = VerticalAlignment.Center });
                probes.Children.Add(row);
                var http = Json.OptionalInteger(probe, "http_status");
                probes.Children.Add(ContextCaption(Json.OptionalText(probe, "error")
                    ?? Localized.Format("HTTP {0} · {1} ms", http?.ToString(CultureInfo.InvariantCulture) ?? "—", Json.Integer(probe, "elapsed_ms"))));
            }
            AutomationProperties.SetAutomationId(probes, "ech-test-result");
            await ShowDialogAsync(Localized.Get("检测 ECH"), probes);
        }));
        AutomationProperties.SetAutomationId(test, "ech-test");
        var body = new StackPanel { Spacing = 16 };
        body.Children.Add(toggle);
        body.Children.Add(ContextCaption(detail));
        body.Children.Add(test);
        return new Arc.ExpandableCard
        {
            Header = Localized.Get("官方连接 · ECH"),
            Description = enabled ? Localized.Get("已启用") : Localized.Get("已关闭"),
            IsExpanded = true,
            Content = body
        };
    }

    // Quota reads call remote providers, so they start when the section is first opened.
    private Control SettingsQuota(CancellationToken token)
    {
        var host = new StackPanel { Spacing = 16 };
        var loaded = false;
        async Task LoadAsync()
        {
            host.Children.Clear();
            host.Children.Add(new Arc.Skeleton { Lines = 3, Label = Localized.Get("正在读取…") });
            try
            {
                var providers = Json.Rows(Json.Field(await cli.QueryAsync(["provider", "list", "--json"], token), "providers"))
                    .Where(provider => Json.Flag(provider, "enabled")).ToArray();
                host.Children.Clear();
                if (providers.Length == 0)
                {
                    host.Children.Add(new Arc.EmptyState
                    {
                        Size = Arc.ControlSize.Small,
                        Title = Localized.Get("没有已启用的供应商"),
                        Description = Localized.Get("先添加并启用供应商，再查看额度。")
                    });
                    return;
                }
                foreach (var provider in providers)
                {
                    var section = new StackPanel { Spacing = 8 };
                    section.Children.Add(new TextBlock { Text = Json.Text(provider, "display_name"), FontWeight = FontWeight.SemiBold });
                    section.Children.Add(new Arc.Skeleton { Lines = 2, Label = Localized.Get("正在读取…") });
                    host.Children.Add(section);
                    try
                    {
                        var rows = Json.Rows(await cli.QueryAsync(["quota", "--provider", Json.Text(provider, "id"), "--json"], token, TimeSpan.FromSeconds(60)));
                        section.Children.RemoveAt(1);
                        if (rows.Length == 0) section.Children.Add(ContextCaption(Localized.Get("此供应商没有额度信息。")));
                        foreach (var row in rows) section.Children.Add(SettingsQuotaRow(row));
                    }
                    catch (Exception error) when (error is CliFailure or InvalidOperationException)
                    {
                        section.Children.RemoveAt(1);
                        section.Children.Add(new Arc.Alert { Tone = Arc.AlertTone.Warning, Title = Localized.Get("额度读取失败"), Content = new TextBlock { Text = error.Message, TextWrapping = TextWrapping.Wrap } });
                    }
                }
            }
            catch (OperationCanceledException) { }
            catch (Exception error) when (error is CliFailure or InvalidOperationException)
            {
                host.Children.Clear();
                var retry = new Arc.Button { Content = Localized.Get("重试"), Variant = Arc.ButtonVariant.Secondary, HorizontalAlignment = HorizontalAlignment.Left };
                retry.Click += async (_, _) => await LoadAsync();
                host.Children.Add(ContextAlert(Localized.Get("额度读取失败"), error.Message, retry));
            }
        }
        var card = new Arc.ExpandableCard
        {
            Header = Localized.Get("额度"),
            Description = Localized.Get("展开后读取各供应商的额度与重置时间。"),
            Content = host
        };
        card.ExpandedChanged += async (_, _) =>
        {
            if (!card.IsExpanded || loaded) return;
            loaded = true;
            await LoadAsync();
        };
        return card;
    }

    private static Control SettingsQuotaRow(JsonElement row)
    {
        static double? Amount(JsonElement source, string field)
        {
            if (!source.TryGetProperty(field, out var value) || value.ValueKind == JsonValueKind.Null) return null;
            if (value.ValueKind != JsonValueKind.Number || !value.TryGetDouble(out var number) || !double.IsFinite(number) || number < 0)
                throw new InvalidOperationException(Localized.Format("CLI 返回了无效的 {0} 数值。", field));
            return number;
        }
        var used = Amount(row, "used") ?? Amount(row, "value");
        var limit = Amount(row, "limit");
        var remaining = Amount(row, "remaining");
        var percent = Json.OptionalText(row, "unit") == "percent";
        var currency = percent ? "" : Json.OptionalText(row, "currency") ?? "";
        var label = Json.OptionalText(row, "quota_id") switch
        {
            "five_hour" => Localized.Get("5 小时"),
            "weekly" => Localized.Get("每周"),
            "monthly" => Localized.Get("每月"),
            "balance" => Localized.Get("余额"),
            _ => Json.OptionalText(row, "label") ?? Json.OptionalText(row, "quota_id") ?? Localized.Get("额度")
        };
        if (Json.OptionalText(row, "error") is { } error)
            return new Arc.Alert { Tone = Arc.AlertTone.Warning, Title = label, Content = new TextBlock { Text = error, TextWrapping = TextWrapping.Wrap } };
        double? ratio = limit is > 0 && used is { } consumed ? consumed / limit.Value
            : limit is > 0 && remaining is { } left ? Math.Max(0, (limit.Value - left) / limit.Value) : null;
        var value = used is { } amount && limit is { } maximum ? $"{amount:0.##} / {maximum:0.##} {currency}".TrimEnd()
            : remaining is { } balance ? Localized.Format("余额 {0:0.##} {1}", balance, currency).TrimEnd()
            : used is { } spent ? Localized.Format("已用 {0:0.##} {1}", spent, currency).TrimEnd() : Localized.Get("额度未知");
        var note = Json.OptionalText(row, "reset_at") is { } reset && DateTimeOffset.TryParse(reset, CultureInfo.InvariantCulture, out var resetAt)
            ? Localized.Format("{0:MM-dd HH:mm} 重置", resetAt.ToLocalTime())
            : !percent && remaining is { } rest && limit is > 0 ? Localized.Format("剩余 {0:0.##} {1}", rest, currency).TrimEnd() : null;
        var body = new StackPanel { Spacing = 4 };
        if (ratio is { } fraction)
            body.Children.Add(new Arc.Progress { Label = label, Minimum = 0, Maximum = 100, Value = Math.Min(100, fraction * 100), IsValueVisible = true });
        else body.Children.Add(new TextBlock { Text = label });
        body.Children.Add(ContextCaption(note is null ? value : $"{value} · {note}"));
        return body;
    }

    private async Task<Control> SettingsKeysAsync(CancellationToken token)
    {
        var keys = Json.Rows(Json.Field(await cli.QueryAsync(["config", "keys", "list", "--json"], token), "keys"))
            .Select(row => new SettingsKey(Json.Text(row, "id"), Json.Text(row, "name"), Json.Text(row, "masked_key"),
                Json.OptionalText(row, "created_at") is { Length: > 0 } created && DateTimeOffset.TryParse(created, CultureInfo.InvariantCulture, out var time)
                    ? time.ToLocalTime().ToString("yyyy-MM-dd HH:mm", CultureInfo.CurrentCulture)
                    : Localized.Get("早期版本生成"))).ToArray();

        var name = new Arc.Input { Label = Localized.Get("名称"), Placeholder = Localized.Get("例如：远程客户端"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var create = SettingsButton(Localized.Get("生成密钥"), () => SettingsGuardAsync(Localized.Get("生成密钥"), async () =>
        {
            var keyName = name.Text?.Trim() ?? "";
            if (keyName.Length == 0) { name.ErrorMessage = Localized.Get("请输入密钥名称。"); return; }
            name.ErrorMessage = null;
            var output = await operations.ChangeAsync(["config", "keys", "create", "--name", keyName, "--json"], windowToken);
            using var document = JsonDocument.Parse(output);
            var secret = Json.Text(document.RootElement, "key");
            var shown = new StackPanel { Spacing = 12, MinWidth = 440 };
            shown.Children.Add(ContextCaption(Localized.Format("新密钥「{0}」只显示这一次，请立即保存。", Json.Text(document.RootElement, "name"))));
            shown.Children.Add(new Arc.Input { Text = secret, IsReadOnly = true, Label = Localized.Get("访问密钥"), HorizontalAlignment = HorizontalAlignment.Stretch });
            shown.Children.Add(new Arc.CopyButton { Value = secret, Label = Localized.Get("复制"), CopiedLabel = Localized.Get("已复制"), HorizontalAlignment = HorizontalAlignment.Left });
            await ShowDialogAsync(Localized.Get("新访问密钥"), shown);
            await RefreshPageAsync();
        }), Arc.ButtonVariant.Primary);

        var body = new StackPanel { Spacing = 16 };
        var form = new Grid { ColumnSpacing = 12 };
        form.ColumnDefinitions.Add(new ColumnDefinition(1, GridUnitType.Star));
        form.ColumnDefinitions.Add(new ColumnDefinition(GridLength.Auto));
        create.VerticalAlignment = VerticalAlignment.Bottom;
        Grid.SetColumn(create, 1);
        form.Children.Add(name);
        form.Children.Add(create);
        body.Children.Add(form);

        if (keys.Length == 0)
        {
            body.Children.Add(new Arc.EmptyState
            {
                Size = Arc.ControlSize.Small,
                Title = Localized.Get("还没有访问密钥"),
                Description = Localized.Get("生成后即可从公网访问网关。")
            });
        }
        else
        {
            var table = new Arc.Table
            {
                Caption = Localized.Get("访问密钥"),
                ItemsSource = keys,
                Height = Math.Clamp(keys.Length * 44 + 60, 140, 360)
            };
            table.Columns.Add(Arc.DataTableColumn.Create<SettingsKey>(Localized.Get("名称"), key => key.Name));
            table.Columns.Add(Arc.DataTableColumn.Create<SettingsKey>(Localized.Get("密钥"), key => key.Masked));
            table.Columns.Add(Arc.DataTableColumn.Create<SettingsKey>(Localized.Get("创建时间"), key => key.Created));
            table.Columns.Add(new Arc.DataTableColumn(Localized.Get("操作"), _ => "")
            {
                IsSortable = false,
                CellTemplate = new FuncDataTemplate<SettingsKey>((_, _) =>
                {
                    var revoke = new Arc.Button { Content = Localized.Get("吊销"), Variant = Arc.ButtonVariant.Danger, Size = Arc.ControlSize.Small };
                    // Rows are recycled, so the key is read from the cell's current data context.
                    revoke.Click += async (sender, _) =>
                    {
                        if (((Control)sender!).DataContext is not SettingsKey key) return;
                        if (await ConfirmAsync(Localized.Get("吊销访问密钥"), Localized.Format("使用「{0}」的客户端会立即收到 401。", key.Name)))
                            await RunActionAsync(["config", "keys", "revoke", key.Id, "--json"]);
                    };
                    return revoke;
                })
            });
            body.Children.Add(table);
        }
        return new Arc.ExpandableCard
        {
            Header = Localized.Get("访问密钥"),
            Description = Localized.Format("{0} 个密钥", keys.Length),
            Content = body
        };
    }

    private async Task<Control> SettingsDiagnosticsAsync(CancellationToken token)
    {
        var contract = await cli.QueryAsync(["interface", "--json"], token);
        Json.Interface(contract);
        var paths = Json.Field(contract, "paths");
        var stateDirectory = Json.Text(paths, "state");
        var gatewayLogs = Path.GetDirectoryName(Json.Text(paths, "gateway_log"))
            ?? throw new InvalidOperationException(Localized.Get("CLI 返回的 gateway_log 路径无效。"));

        var doctor = SettingsButton(Localized.Get("运行诊断"), () => SettingsGuardAsync(Localized.Get("诊断"), async () =>
        {
            var report = await cli.QueryAsync(["doctor", "--json"], windowToken, TimeSpan.FromMinutes(3));
            await ShowDialogAsync(Localized.Get("诊断"), SettingsDoctorReport(report));
        }), Arc.ButtonVariant.Primary);
        AutomationProperties.SetAutomationId(doctor, "run-diagnostics");
        var repair = SettingsButton(Localized.Get("自动修复"), async () =>
        {
            if (await ConfirmAsync(Localized.Get("自动修复？"), Localized.Get("将修复权限、过期状态、网关启动、base_url 和模型目录等可安全处理的问题。")))
                await RunActionAsync(["doctor", "--fix", "--quick"]);
        });
        var logs = SettingsButton(Localized.Get("查看网关日志"), () => SettingsGuardAsync(Localized.Get("网关日志"), async () =>
        {
            var text = await cli.RunAsync(["service", "logs", "-n", "200"], windowToken);
            await ShowDialogAsync(Localized.Get("网关日志"), new Arc.Textarea
            {
                Text = string.IsNullOrWhiteSpace(text) ? Localized.Get("暂无日志。") : text,
                IsReadOnly = true,
                TextWrapping = TextWrapping.NoWrap,
                FontFamily = new FontFamily("Menlo, Consolas, monospace"),
                FrameMinHeight = 320,
                MinWidth = 640
            });
        }));
        var actions = new WrapPanel { Orientation = Orientation.Horizontal };
        foreach (var control in new Control[]
        {
            doctor, repair, logs,
            SettingsButton(Localized.Get("打开网关日志目录"), () => SettingsOpenAsync(gatewayLogs)),
            SettingsButton(Localized.Get("打开配置目录"), () => SettingsOpenAsync(stateDirectory)),
            SettingsButton(Localized.Get("打开界面日志目录"), () =>
            {
                Directory.CreateDirectory(Diagnostics.DirectoryPath);
                return SettingsOpenAsync(Diagnostics.DirectoryPath);
            })
        })
        {
            control.Margin = new Thickness(0, 0, 8, 8);
            actions.Children.Add(control);
        }
        return new Arc.ExpandableCard
        {
            Header = Localized.Get("诊断与日志"),
            Description = Localized.Format("CLI {0}", Json.Text(contract, "version")),
            IsExpanded = true,
            Content = actions
        };
    }

    private Task SettingsOpenAsync(string directory) => SettingsGuardAsync(Localized.Get("打开目录"), () =>
    {
        if (!Path.IsPathFullyQualified(directory) || !Directory.Exists(directory))
            throw new DirectoryNotFoundException(Localized.Format("目录不存在：{0}", directory));
        var opener = OperatingSystem.IsMacOS() ? "open" : OperatingSystem.IsWindows() ? "explorer.exe" : "xdg-open";
        var start = new ProcessStartInfo(opener) { UseShellExecute = false };
        start.ArgumentList.Add(directory);
        using var process = Process.Start(start) ?? throw new IOException(Localized.Format("无法打开目录：{0}", directory));
        return Task.CompletedTask;
    });

    private static Control SettingsDoctorReport(JsonElement report)
    {
        var summary = Json.Field(report, "summary");
        var body = new StackPanel { Spacing = 10, MinWidth = 560 };
        var totals = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        totals.Children.Add(new Arc.Badge { Content = Localized.Format("正常 {0}", Json.Integer(summary, "ok")), Tone = Arc.BadgeTone.Success });
        totals.Children.Add(new Arc.Badge { Content = Localized.Format("警告 {0}", Json.Integer(summary, "warnings")), Tone = Arc.BadgeTone.Warning });
        totals.Children.Add(new Arc.Badge { Content = Localized.Format("错误 {0}", Json.Integer(summary, "errors")), Tone = Arc.BadgeTone.Danger });
        body.Children.Add(totals);
        void Add(string title, string status, string message, string? detail, string? hint)
        {
            var row = new StackPanel { Spacing = 2 };
            var head = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
            head.Children.Add(new Arc.Badge
            {
                Content = status switch { "ok" => Localized.Get("正常"), "warning" => Localized.Get("警告"), "error" => Localized.Get("错误"), var other => other },
                Size = Arc.BadgeSize.Small,
                Tone = status switch { "ok" => Arc.BadgeTone.Success, "warning" => Arc.BadgeTone.Warning, "error" => Arc.BadgeTone.Danger, _ => Arc.BadgeTone.Neutral }
            });
            head.Children.Add(new TextBlock { Text = title, FontWeight = FontWeight.SemiBold, VerticalAlignment = VerticalAlignment.Center });
            row.Children.Add(head);
            row.Children.Add(ContextCaption(message));
            if (!string.IsNullOrWhiteSpace(detail)) row.Children.Add(ContextCaption(detail));
            if (!string.IsNullOrWhiteSpace(hint)) row.Children.Add(ContextCaption(Localized.Format("建议：{0}", hint)));
            body.Children.Add(row);
        }
        foreach (var check in Json.Rows(Json.Field(report, "checks")))
            Add(Json.Text(check, "name"), Json.Text(check, "status"), Json.Text(check, "message"),
                Json.OptionalText(check, "detail"), Json.OptionalText(check, "fix_hint"));
        foreach (var provider in Json.Rows(Json.Field(report, "providers")))
            Add(Json.Text(provider, "display_name"), Json.Text(provider, "status"), Json.Text(provider, "message"),
                Json.OptionalText(provider, "detail"), null);
        var scroll = new ScrollViewer { MaxHeight = 460, Content = body };
        AutomationProperties.SetAutomationId(scroll, "doctor-report");
        return scroll;
    }

    private Control SettingsBackup()
    {
        var filter = new FilePickerFileType("Codex Mixin backup") { Patterns = ["*.b64"] };
        var export = SettingsButton(Localized.Get("导出配置"), () => SettingsGuardAsync(Localized.Get("导出配置"), async () =>
        {
            if (!await ConfirmAsync(Localized.Get("导出包含凭据的备份？"), Localized.Get("Base64 不是加密。备份包含供应商密钥，请保存到安全位置。"))) return;
            var file = await StorageProvider.SaveFilePickerAsync(new FilePickerSaveOptions
            {
                Title = Localized.Get("选择保存位置"),
                SuggestedFileName = "codex-mixin-config",
                DefaultExtension = "b64",
                FileTypeChoices = [filter]
            });
            if (file is null) return;
            await RunActionAsync(["config", "export", file.TryGetLocalPath() ?? throw new IOException(Localized.Get("所选位置不是本地文件。"))]);
        }));
        var import = SettingsButton(Localized.Get("导入配置"), () => SettingsGuardAsync(Localized.Get("导入配置"), async () =>
        {
            var files = await StorageProvider.OpenFilePickerAsync(new FilePickerOpenOptions
            {
                Title = Localized.Get("导入配置"),
                AllowMultiple = false,
                FileTypeFilter = [filter]
            });
            if (files.Count == 0) return;
            var path = files[0].TryGetLocalPath() ?? throw new IOException(Localized.Get("所选位置不是本地文件。"));
            if (await ConfirmAsync(Localized.Get("替换当前配置？"), Localized.Get("导入会替换现有供应商与密钥，并应用到已接入的客户端。")))
                await RunActionAsync(["config", "import", path], apply: true);
        }));
        AutomationProperties.SetAutomationId(export, "export-config");
        AutomationProperties.SetAutomationId(import, "import-config");
        var actions = new WrapPanel { Orientation = Orientation.Horizontal };
        export.Margin = new Thickness(0, 0, 8, 8);
        actions.Children.Add(export);
        actions.Children.Add(import);
        var body = new StackPanel { Spacing = 12 };
        body.Children.Add(actions);
        body.Children.Add(ContextCaption(Localized.Get("备份为 Base64 文本，包含供应商密钥；请按凭据文件保管。")));
        return new Arc.ExpandableCard
        {
            Header = Localized.Get("配置备份"),
            Description = Localized.Get("导出或恢复供应商与密钥配置"),
            IsExpanded = true,
            Content = body
        };
    }
}
