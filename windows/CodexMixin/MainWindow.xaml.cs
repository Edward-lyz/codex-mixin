using System.Diagnostics;
using System.Text.Json;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.ApplicationModel.DataTransfer;
using Windows.Storage.Pickers;
using Windows.System;
using WinRT.Interop;

namespace CodexMixin;

// The XAML compiler requires a public root type; this is not a business API.
public sealed partial class MainWindow : Window
{
    private readonly CancellationTokenSource lifetime = new();
    private readonly DispatcherTimer timer = new() { Interval = TimeSpan.FromSeconds(30) };
    private CliClient? cli;
    private Operations? operations;
    private NativeShell? shell;
    private JsonElement metadata;
    private string page = "overview";
    private string? providerId;
    private bool ready;
    private bool busy;
    private bool exiting;
    private bool dialogOpen;
    private bool initialized;

    public MainWindow()
    {
        InitializeComponent();
        AutomationProperties.SetAutomationId((DependencyObject)Navigation.SettingsItem, "nav-settings");
        Title = "Codex Mixin";
        AppWindow.Resize(new Windows.Graphics.SizeInt32(1120, 800));
        if (AppWindowTitleBar.IsCustomizationSupported())
        {
            ExtendsContentIntoTitleBar = true;
            SetTitleBar(TitleBar);
        }
        SystemBackdrop = new MicaBackdrop();
        var handle = WindowNative.GetWindowHandle(this);
        try { shell = new NativeShell(handle, OnTrayCommand); }
        catch (Exception error) { ShowNotice("系统托盘不可用", error.Message, InfoBarSeverity.Warning); }
        AppWindow.Closing += (_, args) =>
        {
            if (!exiting && shell != null) { args.Cancel = true; shell.Hide(); }
        };
        Closed += (_, _) =>
        {
            exiting = true;
            timer.Stop();
            lifetime.Cancel();
            shell?.Dispose();
        };
        Root.Loaded += async (_, _) =>
        {
            if (initialized) return;
            initialized = true;
            ready = true;
            await ExecuteUiAsync("连接 CLI", async token =>
            {
                cli = CliClient.Locate();
                operations = new Operations(cli);
                metadata = await cli.QueryAsync(["interface", "--json"], token);
                Json.Interface(metadata);
                await LoadPageAsync(token);
            });
            timer.Start();
            if (Environment.GetCommandLineArgs().Contains("--background") && shell != null) shell.Hide();
        };
        timer.Tick += async (_, _) =>
        {
            if (!busy && !dialogOpen && page == "overview" && AppWindow.IsVisible && cli != null)
                await ExecuteUiAsync("刷新状态", LoadPageAsync);
        };
        var refresh = new KeyboardAccelerator { Key = VirtualKey.R, Modifiers = VirtualKeyModifiers.Control };
        refresh.Invoked += async (_, args) => { args.Handled = true; await ReloadAsync(); };
        Root.KeyboardAccelerators.Add(refresh);
    }

    internal void ShowMain()
    {
        shell?.Show();
        AppWindow.Show();
        Activate();
    }

    private void OnTrayCommand(string command) => DispatcherQueue.TryEnqueue(async () =>
    {
        if (command == "tray-error")
        {
            ShowMain();
            ShowNotice("系统托盘不可用", "Explorer 重启后无法恢复托盘，请重新启动应用。", InfoBarSeverity.Warning);
            return;
        }
        if (command == "exit") { await ExitAsync(); return; }
        if (busy) { ShowMain(); return; }
        if (command is "start" or "stop")
        {
            await ChangeAsync(command == "start" ? "启动网关" : "停止网关",
                ["service", command, "--managed", "--json"]);
            return;
        }
        ShowMain();
        if (command == "settings") Navigation.SelectedItem = Navigation.SettingsItem;
        else Navigation.SelectedItem = Navigation.MenuItems[0];
    });

    private async Task ExitAsync()
    {
        if (busy)
        {
            ShowMain();
            ShowNotice("操作仍在进行", "请等待当前操作完成后退出。关闭窗口可以收起到托盘。", InfoBarSeverity.Warning);
            return;
        }
        if (await ConfirmAsync("退出 Codex Mixin？", "退出界面后，本地网关会继续按 CLI 的服务设置运行。", "退出"))
        {
            exiting = true;
            Close();
        }
    }

    private async void OnNavigationChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (!ready || busy) return;
        page = args.IsSettingsSelected ? "settings" : (args.SelectedItem as NavigationViewItem)?.Tag as string ?? "overview";
        await ReloadAsync();
    }

    private async void OnRefresh(object sender, RoutedEventArgs args) => await ReloadAsync();

    private Task ReloadAsync() => ExecuteUiAsync("刷新", async token =>
    {
        cli ??= CliClient.Locate();
        operations ??= new Operations(cli);
        metadata = await cli.QueryAsync(["interface", "--json"], token);
        Json.Interface(metadata);
        await LoadPageAsync(token);
    });

    private async Task ExecuteUiAsync(string label, Func<CancellationToken, Task> work)
    {
        if (busy || exiting) return;
        busy = true;
        Navigation.IsEnabled = false;
        Activity.IsActive = true;
        Activity.Visibility = Visibility.Visible;
        StatusText.Text = $"{label}…";
        using var cancellation = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
        try
        {
            await work(cancellation.Token);
            if (!exiting) StatusText.Text = $"{label}完成 · {DateTime.Now:HH:mm:ss}";
        }
        catch (Exception error)
        {
            if (exiting) return;
            var title = error is CliFailure { Committed: true } ? "配置已保存，后续操作失败" : $"{label}失败";
            StatusText.Text = title;
            ShowNotice(title, error.Message, InfoBarSeverity.Error);
            // Never leave a failed status query showing a previously healthy gateway.
            if (page == "overview")
            {
                PageBody.Children.Clear();
                PageBody.Children.Add(Card("状态未知", "无法读取当前状态，请刷新重试。", Text(error.Message)));
            }
        }
        finally
        {
            busy = false;
            Navigation.IsEnabled = true;
            Activity.IsActive = false;
            Activity.Visibility = Visibility.Collapsed;
        }
    }

    private Task ChangeAsync(string label, string[] args, bool apply = false,
        IReadOnlyDictionary<string, string>? secrets = null) => ExecuteUiAsync(label, async token =>
    {
        var service = operations ?? throw new InvalidOperationException("CLI 尚未连接，请先刷新。");
        var progress = new Progress<string>(message => StatusText.Text = message);
        var output = await service.ChangeAsync(args, token, secrets, progress, apply);
        ShowNotice(label, string.IsNullOrWhiteSpace(output) ? "操作完成。" : output.Trim(), InfoBarSeverity.Success);
        try { await LoadPageAsync(token); }
        catch (Exception error)
        {
            ShowNotice($"{label}完成，状态刷新失败", error.Message, InfoBarSeverity.Warning);
            if (page == "overview") PageBody.Children.Clear();
        }
    });

    private async Task LoadPageAsync(CancellationToken token)
    {
        PageBody.Children.Clear();
        (PageTitle.Text, PageDescription.Text) = page switch
        {
            "providers" => ("供应商与模型", "管理服务连接与向 Agent 提供的模型"),
            "agents" => ("Agent 接入", "检测、接入与升级由本地 CLI 执行"),
            "usage" => ("用量与额度", "真实请求用量、缓存命中与供应商额度"),
            "fusion" => ("Fusion", "组合 Panel、Judge 与 Final 模型"),
            "settings" => ("设置", "外观、启动、备份与诊断"),
            _ => ("概览", "模型服务、连接状态与常用操作")
        };
        switch (page)
        {
            case "providers": await ProvidersAsync(token); break;
            case "agents": await AgentsAsync(token); break;
            case "usage": await UsageAsync(token); break;
            case "fusion": await FusionAsync(token); break;
            case "settings": await SettingsAsync(token); break;
            default: await OverviewAsync(token); break;
        }
    }

    private CliClient Client => cli ?? throw new InvalidOperationException("CLI 尚未连接，请刷新。");

    private async Task OverviewAsync(CancellationToken token)
    {
        var status = await Client.QueryAsync(["service", "status", "--json"], token);
        var state = Json.Gateway(status);
        var endpoint = Json.OptionalText(status, "endpoint");
        var buttons = Row();
        buttons.Children.Add(Button("启动网关", () => ChangeAsync("启动网关", ["service", "start", "--managed", "--json"]), true));
        buttons.Children.Add(Button("停止网关", () => ChangeAsync("停止网关", ["service", "stop", "--managed", "--json"])));
        buttons.Children.Add(Button("重启", () => ChangeAsync("重启网关", ["service", "restart", "--managed", "--json"])));
        if (endpoint != null) buttons.Children.Add(Button("复制地址", () =>
        {
            var content = new DataPackage(); content.SetText(endpoint); Clipboard.SetContent(content);
            StatusText.Text = "接口地址已复制";
            return Task.CompletedTask;
        }));
        PageBody.Children.Add(Card($"本地网关 · {state}", endpoint ?? "尚无运行中的接口地址", buttons));
        if (Json.OptionalText(status, "error") is { } statusError)
            PageBody.Children.Add(Card("服务诊断", statusError));
        var providers = Json.Rows(Json.Field(await Client.QueryAsync(["provider", "list", "--json"], token), "providers"));
        if (providers.Length == 0) PageBody.Children.Add(Card("开始连接模型服务", "先添加一个供应商，再选择模型并接入 Agent。",
            Button("添加供应商", AddProviderAsync, true)));
        foreach (var provider in providers)
        {
            var selected = Json.Strings(Json.Field(provider, "selected_models"));
            PageBody.Children.Add(Card(Json.Text(provider, "display_name"),
                $"{Readiness(provider)} · {selected.Length} 个模型", Text(string.Join("  ·  ", selected))));
        }
        PageBody.Children.Add(Card("使用 Windows 原生界面", "WinUI 3 · Fluent Design · Rust 本地网关",
            Text($"CLI {Json.Text(metadata, "version")} · 关闭窗口收起到托盘，右键托盘可退出。")));
    }

    private static string Readiness(JsonElement provider) => !Json.Flag(provider, "enabled") ? "已停用" :
        Json.Text(provider, "readiness") switch { "healthy" => "正常", "degraded" => "降级", var value => value };

    private async Task ProvidersAsync(CancellationToken token)
    {
        var providers = Json.Rows(Json.Field(await Client.QueryAsync(["provider", "list", "--json"], token), "providers"));
        PageBody.Children.Add(Button("添加供应商", AddProviderAsync, true));
        if (providers.Length == 0) { PageBody.Children.Add(Card("暂无供应商", "添加供应商后，CLI 会发现可用模型。")); return; }
        var picker = new ComboBox { Header = "供应商", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var provider in providers)
            picker.Items.Add(new ComboBoxItem { Content = $"{Json.Text(provider, "display_name")} · {Readiness(provider)}", Tag = Json.Text(provider, "id") });
        var detail = new StackPanel { Spacing = 16 };
        picker.SelectionChanged += (_, _) =>
        {
            providerId = (picker.SelectedItem as ComboBoxItem)?.Tag as string;
            detail.Children.Clear();
            var selected = providers.First(row => Json.Text(row, "id") == providerId);
            RenderProvider(selected, detail);
        };
        PageBody.Children.Add(picker);
        PageBody.Children.Add(detail);
        picker.SelectedIndex = Math.Max(0, Array.FindIndex(providers, row => Json.Text(row, "id") == providerId));
    }

    private void RenderProvider(JsonElement provider, StackPanel detail)
    {
        var id = Json.Text(provider, "id");
        var official = Json.Text(provider, "kind") == "official";
        var toolbar = Row();
        toolbar.Children.Add(Button("刷新模型", () => ChangeAsync("刷新模型", ["provider", "discover", id], apply: true)));
        if (!official)
        {
            toolbar.Children.Add(Button("编辑连接", () => EditProviderAsync(provider)));
            toolbar.Children.Add(Button("验证连接", () => ChangeAsync("验证连接", ["provider", "test", id, "--json"])));
            toolbar.Children.Add(Button(Json.Flag(provider, "enabled") ? "停用" : "启用", () =>
                ChangeAsync("更新供应商状态", ["provider", Json.Flag(provider, "enabled") ? "disable" : "enable", id], apply: true)));
            toolbar.Children.Add(Button("删除", async () =>
            {
                if (await ConfirmAsync("删除供应商？", $"将移除 {Json.Text(provider, "display_name")} 及其模型配置。", "删除"))
                    await ChangeAsync("删除供应商", ["provider", "remove", id], apply: true);
            }));
        }
        detail.Children.Add(Card(Json.Text(provider, "display_name"),
            $"{Json.Text(provider, "protocol")} · {Json.Text(provider, "base_url")}", toolbar));
        var issues = Json.Strings(Json.Field(provider, "readiness_issues"));
        if (issues.Length > 0) detail.Children.Add(Card("需要处理", string.Join("\n", issues)));
        if (Json.OptionalText(provider, "last_model_refresh_error") is { } refreshError)
            detail.Children.Add(Card("上次模型刷新失败", refreshError));
        var models = Json.Rows(Json.Field(provider, "cached_models"));
        var selected = Json.Strings(Json.Field(provider, "selected_models")).ToHashSet();
        var list = new StackPanel { Spacing = 10 };
        var search = new TextBox { PlaceholderText = "搜索模型", Header = "可用模型" };
        AutomationProperties.SetName(search, "搜索可用模型");
        var entries = new List<(string Id, CheckBox Check, Grid Row, NumberBox Context)>();
        var overrides = Json.Field(provider, "model_context_overrides");
        foreach (var model in models)
        {
            var modelId = Json.Text(model, "id");
            var check = new CheckBox { Content = Json.OptionalText(model, "display_name") ?? modelId, IsChecked = selected.Contains(modelId) };
            var context = new NumberBox { PlaceholderText = "默认上下文", Width = 180, Minimum = 1,
                Value = overrides.TryGetProperty(modelId, out var saved) ? saved.GetDouble() : double.NaN };
            AutomationProperties.SetName(context, $"{modelId} 上下文 Token 数，留空使用默认值");
            var row = new Grid { ColumnSpacing = 12 };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            row.Children.Add(check); Grid.SetColumn(context, 1); row.Children.Add(context);
            ToolTipService.SetToolTip(check, modelId);
            entries.Add((modelId, check, row, context));
            list.Children.Add(row);
        }
        search.TextChanged += (_, _) =>
        {
            foreach (var entry in entries)
                entry.Row.Visibility = entry.Id.Contains(search.Text, StringComparison.OrdinalIgnoreCase) ? Visibility.Visible : Visibility.Collapsed;
        };
        var body = new StackPanel { Spacing = 16 };
        body.Children.Add(search);
        body.Children.Add(list);
        var modelActions = Row();
        modelActions.Children.Add(Button("保存模型选择", async () =>
        {
            var args = new List<string> { "provider", "select", id };
            foreach (var entry in entries)
            {
                if (entry.Check.IsChecked == true) args.AddRange(["--model", entry.Id]);
                if (!double.IsNaN(entry.Context.Value))
                {
                    if (!double.IsFinite(entry.Context.Value) || entry.Context.Value < 1 || entry.Context.Value != Math.Truncate(entry.Context.Value))
                        throw new InvalidOperationException("上下文 Token 数必须是正整数。");
                    args.AddRange(["--model-context", $"{entry.Id}={entry.Context.Value:0}"]);
                }
                else if (overrides.TryGetProperty(entry.Id, out _)) args.AddRange(["--clear-model-context", entry.Id]);
            }
            await ChangeAsync("保存模型选择", args.ToArray(), apply: true);
        }, true));
        modelActions.Children.Add(Button("探测能力", () => ChangeAsync("探测模型能力", ["provider", "probe", id], apply: true)));
        modelActions.Children.Add(Button("测速", () => ChangeAsync("启动模型测速",
            ["benchmark", "start", "--provider", id, "--timeout-seconds", "120"])));
        body.Children.Add(modelActions);
        detail.Children.Add(Card($"模型 · {selected.Count} / {models.Length}", "勾选向 Agent 提供的模型；搜索不会改变选择。", body));
    }

    private Task AddProviderAsync() => ProviderDialogAsync(null);
    private Task EditProviderAsync(JsonElement provider) => ProviderDialogAsync(provider);

    private async Task ProviderDialogAsync(JsonElement? provider)
    {
        var form = new StackPanel { Spacing = 12, MinWidth = 360 };
        var preset = Choice("供应商类型", ["baidu-oneapi", "openrouter", "deepseek", "opencode-go", "aws-bedrock", "custom"],
            provider is { } stored ? Json.OptionalText(stored, "preset_id") ?? "custom" : "baidu-oneapi");
        preset.IsEnabled = provider == null;
        var id = Input("供应商 ID", provider is { } p ? Json.Text(p, "id") : "");
        id.IsEnabled = provider == null;
        var name = Input("显示名称", provider is { } n ? Json.Text(n, "display_name") : "");
        var url = Input("Base URL（预设可留空）", provider is { } u ? Json.Text(u, "base_url") : "");
        var website = Input("供应商网站（可选）", provider is { } site ? Json.OptionalText(site, "website_url") ?? "" : "");
        var imagePath = Input("图像生成路径（可选）", provider is { } image ? Json.OptionalText(image, "image_generation_path") ?? "" : "");
        var apiPath = Input("API 路径（custom，可选）", provider is { } api ? Json.OptionalText(api, "api_path") ?? "" : "");
        var key = new PasswordBox { Header = provider == null ? "API Key" : "新 API Key（留空保留）" };
        var protocol = Choice("协议（custom）", ["自动检测", "open_ai_responses", "open_ai_chat", "anthropic_messages"],
            provider is { } pr ? Json.Text(pr, "protocol") : "自动检测");
        var username = Input("OneAPI 额度用户名", provider is { } q ? Json.OptionalText(q, "quota_username") ?? "" : "");
        var workspace = Input("OpenCode Go workspace ID", provider is { } w ? Json.OptionalText(w, "quota_workspace_id") ?? "" : "");
        var cookie = new PasswordBox { Header = "OpenCode Go Cookie（留空保留）" };
        var accessKey = new PasswordBox { Header = "AWS Access Key ID（留空保留）" };
        var secretKey = new PasswordBox { Header = "AWS Secret Access Key（留空保留）" };
        var session = new PasswordBox { Header = "AWS Session Token（可选）" };
        var region = Input("AWS Region", provider is { } r ? Json.OptionalText(r, "aws_region") ?? "us-east-1" : "us-east-1");
        var bridge = Choice("OneAPI 认证桥接", ["disabled", "ducx_loopback"],
            provider is { } b ? Json.OptionalText(b, "baidu_auth_bridge") ?? "disabled" : "disabled");
        var report = new CheckBox { Content = "OneAPI 代码上报", IsChecked = provider is { } br &&
            Json.Field(br, "baidu_code_report").ValueKind == JsonValueKind.True };
        var auxiliary = new CheckBox { Content = "作为辅助模型上游", IsChecked = provider is { } a && Json.Flag(a, "auxiliary_model_upstream") };
        foreach (var element in new FrameworkElement[] { preset, id, name, url, website, protocol, apiPath, imagePath, key, username, bridge, report, workspace, cookie, accessKey, secretKey, session, region, auxiliary })
            form.Children.Add(element);
        void Fields()
        {
            var type = (string)preset.SelectedItem;
            key.Visibility = type == "aws-bedrock" ? Visibility.Collapsed : Visibility.Visible;
            foreach (var element in new FrameworkElement[] { accessKey, secretKey, session, region }) element.Visibility = type == "aws-bedrock" ? Visibility.Visible : Visibility.Collapsed;
            foreach (var element in new FrameworkElement[] { username, bridge, report }) element.Visibility = type == "baidu-oneapi" ? Visibility.Visible : Visibility.Collapsed;
            foreach (var element in new FrameworkElement[] { workspace, cookie }) element.Visibility = type == "opencode-go" ? Visibility.Visible : Visibility.Collapsed;
            protocol.Visibility = type == "custom" ? Visibility.Visible : Visibility.Collapsed;
            apiPath.Visibility = protocol.Visibility;
        }
        preset.SelectionChanged += (_, _) => Fields(); Fields();
        var dialog = Dialog(provider == null ? "添加供应商" : "编辑供应商", new ScrollViewer { Content = form, MaxHeight = 480 });
        dialog.PrimaryButtonText = "保存";
        if (await ShowDialogAsync(dialog) != ContentDialogResult.Primary) return;
        var args = new List<string> { "provider", provider == null ? "add" : "update" };
        if (provider == null) args.AddRange(["--preset", (string)preset.SelectedItem]);
        if (!string.IsNullOrWhiteSpace(id.Text))
        {
            if (provider == null) args.Add("--id");
            args.Add(id.Text.Trim());
        }
        var secrets = new Dictionary<string, string>();
        void Secret(string flag, PasswordBox box, string variable)
        {
            if (box.Password.Length == 0) return;
            secrets[variable] = box.Password; args.AddRange([flag, $"@env:{variable}"]); box.Password = "";
        }
        foreach (var (flag, box) in new[] { ("--display-name", name), ("--base-url", url), ("--website-url", website), ("--image-generation-path", imagePath) })
            if (!string.IsNullOrWhiteSpace(box.Text)) args.AddRange([flag, box.Text.Trim()]);
        var type = (string)preset.SelectedItem;
        if (type == "custom" && (string)protocol.SelectedItem != "自动检测") args.AddRange(["--protocol", (string)protocol.SelectedItem]);
        if (type == "custom" && !string.IsNullOrWhiteSpace(apiPath.Text)) args.AddRange(["--api-path", apiPath.Text.Trim()]);
        if (type == "aws-bedrock")
        {
            Secret("--aws-access-key-id", accessKey, "CM_AWS_ID");
            Secret("--aws-secret-access-key", secretKey, "CM_AWS_SECRET");
            Secret("--aws-session-token", session, "CM_AWS_SESSION");
            args.AddRange(["--aws-region", region.Text.Trim()]);
        }
        else Secret("--key", key, "CM_API_KEY");
        if (type == "baidu-oneapi")
        {
            if (!string.IsNullOrWhiteSpace(username.Text)) args.AddRange(["--quota-username", username.Text.Trim()]);
            args.AddRange(["--baidu-auth-bridge", (string)bridge.SelectedItem, "--baidu-code-report", report.IsChecked == true ? "true" : "false"]);
        }
        if (type == "opencode-go")
        {
            if (!string.IsNullOrWhiteSpace(workspace.Text)) args.AddRange(["--quota-workspace-id", workspace.Text.Trim()]);
            Secret("--quota-auth-cookie", cookie, "CM_QUOTA_COOKIE");
        }
        args.AddRange(["--auxiliary-model-upstream", auxiliary.IsChecked == true ? "true" : "false"]);
        try { await ChangeAsync("保存供应商", args.ToArray(), apply: true, secrets: secrets); }
        finally { secrets.Clear(); }
    }

    private async Task AgentsAsync(CancellationToken token)
    {
        var agents = Json.Rows(Json.Field(await Client.QueryAsync(["agent", "list", "--json"], token), "agents"));
        PageBody.Children.Add(Button("检查版本更新", () => ExecuteUiAsync("检查 Agent 版本", async cancellation =>
        {
            var versions = Json.Rows(Json.Field(await Client.QueryAsync(["agent", "versions", "--json", "--refresh"], cancellation), "agents"));
            var body = new StackPanel { Spacing = 12 };
            foreach (var agent in versions)
            {
                var version = Json.Field(agent, "cli");
                var id = Json.Text(agent, "id");
                var row = Row();
                row.Children.Add(Text($"{id} · 当前 {Json.OptionalText(version, "version") ?? Json.OptionalText(version, "app_version") ?? "未知"} · 最新 {Json.OptionalText(version, "latest") ?? "未知"}"));
                if (Json.Flag(version, "update")) row.Children.Add(Button("升级", () => ChangeAsync("升级 Agent", ["agent", "upgrade", id, "--json"])));
                if (Json.OptionalText(version, "error") is { } error) row.Children.Add(Text(error));
                body.Children.Add(row);
            }
            PageBody.Children.Insert(1, Card("版本检查结果", "每个 Agent 的查询结果由 CLI 返回。", body));
        })));
        foreach (var agent in agents)
        {
            var id = Json.Text(agent, "id");
            var integration = Json.Field(agent, "integration");
            var state = Json.Text(integration, "state");
            var version = Json.Field(agent, "cli");
            var buttons = Row();
            if (state != "unsupported")
            {
                buttons.Children.Add(Button("接入", async () =>
                {
                    var args = new List<string> { "agent", "connect", id };
                    if (id == "codex")
                    {
                        var mode = Choice("接入模式", ["official-account", "custom-only"], "official-account");
                        var dialog = Dialog("接入 Codex", mode); dialog.PrimaryButtonText = "接入";
                        if (await ShowDialogAsync(dialog) != ContentDialogResult.Primary) return;
                        args.AddRange(["--mode", (string)mode.SelectedItem]);
                    }
                    await ChangeAsync("接入 Agent", args.ToArray());
                }, true));
                buttons.Children.Add(Button("断开", async () =>
                {
                    if (await ConfirmAsync("断开 Agent？", "CLI 将恢复或移除此 Agent 的托管配置。", "断开"))
                        await ChangeAsync("断开 Agent", ["agent", "disconnect", id]);
                }));
            }
            buttons.Children.Add(Button("升级", () => ChangeAsync("升级 Agent", ["agent", "upgrade", id, "--json"])));
            var body = new StackPanel { Spacing = 12 };
            body.Children.Add(Text(Json.Text(integration, "detail")));
            if (Json.OptionalText(integration, "error") is { } integrationError) body.Children.Add(Text(integrationError));
            if (Json.OptionalText(version, "error") is { } versionError) body.Children.Add(Text(versionError));
            body.Children.Add(buttons);
            PageBody.Children.Add(Card(Json.Text(agent, "name"),
                $"{(Json.Flag(agent, "detected") ? "已检测到" : "未检测到")} · {state} · {Json.OptionalText(version, "version") ?? Json.OptionalText(version, "app_version") ?? "版本未知"}", body));
        }
    }

    private async Task UsageAsync(CancellationToken token)
    {
        var range = Choice("统计范围", ["今天", "7 天", "30 天", "全部"], "今天");
        var usageBody = new StackPanel { Spacing = 12 };
        async Task ReadUsage(CancellationToken cancellation)
        {
            usageBody.Children.Clear();
            var args = new List<string> { "usage", "--json" };
            if ((string)range.SelectedItem != "全部") args.AddRange(["--days", range.SelectedIndex switch { 1 => "7", 2 => "30", _ => "1" }]);
            try
            {
                var rows = Json.Rows(await Client.QueryAsync(args.ToArray(), cancellation));
                if (rows.Length == 0) usageBody.Children.Add(Text("此时间范围暂无请求记录。"));
                foreach (var row in rows)
                    usageBody.Children.Add(Card($"{Json.Text(row, "provider_id")} / {Json.Text(row, "model_id")}",
                        $"请求 {Json.Field(row, "request_count")} · 输入 {Json.Field(row, "input_tokens")} · 缓存 {Json.Field(row, "cache_read_tokens")} · 输出 {Json.Field(row, "output_tokens")}",
                        Text($"缓存命中 {Metric(row, "cache_hit_percent", "%")} · TTFT {Metric(row, "average_ttft_ms", " ms")} · 输出 {Metric(row, "output_tps", " tokens/s")}")));
            }
            catch (CliFailure error) { usageBody.Children.Add(Card("用量不可用", error.Message)); }
        }
        range.SelectionChanged += async (_, _) => await ExecuteUiAsync("读取用量", ReadUsage);
        PageBody.Children.Add(range); PageBody.Children.Add(usageBody);
        await ReadUsage(token);
        var quotaBody = new StackPanel { Spacing = 12 };
        try
        {
            var rows = Json.Rows(await Client.QueryAsync(["quota", "--json"], token));
            if (rows.Length == 0) quotaBody.Children.Add(Text("暂无可查询的供应商额度。"));
            foreach (var row in rows)
            {
                var label = Json.OptionalText(row, "display_name") ?? Json.OptionalText(row, "provider_id") ?? "额度";
                var error = Json.OptionalText(row, "error");
                quotaBody.Children.Add(Text(error != null ? $"{label} · 查询失败：{error}" :
                    $"{label} · 剩余 {Json.Field(row, "remaining")} {Json.OptionalText(row, "currency") ?? ""}"));
            }
        }
        catch (CliFailure error) { quotaBody.Children.Add(Text($"额度不可用：{error.Message}")); }
        PageBody.Children.Add(Card("供应商额度", "查询失败时展示原因，未知额度不会显示为零。", quotaBody));
    }

    private static string Metric(JsonElement row, string key, string unit)
    {
        var value = Json.Field(row, key);
        return value.ValueKind == JsonValueKind.Null ? "未知" : $"{value.GetDouble():0.##}{unit}";
    }

    private async Task FusionAsync(CancellationToken token)
    {
        var envelope = await Client.QueryAsync(["fusion", "get", "--json"], token);
        var profile = Json.Field(envelope, "profile");
        var options = Json.Rows(Json.Field(await Client.QueryAsync(["fusion", "models", "--json"], token), "models"));
        var ids = options.Select(row => Json.Text(row, "id")).Where(id => !id.StartsWith("mixin/fusion/", StringComparison.Ordinal)).ToArray();
        if (ids.Length == 0) { PageBody.Children.Add(Card("暂无可用模型", "先在供应商中选择模型，再创建 Fusion。")); return; }
        var saved = profile.ValueKind == JsonValueKind.Object;
        var profileId = Input("Profile ID", saved ? Json.Text(profile, "id") : "default");
        var panels = new StackPanel { Spacing = 8 };
        var choices = new List<CheckBox>();
        var selected = saved ? Json.Strings(Json.Field(profile, "panel_models")) : ids.Take(3).ToArray();
        foreach (var id in ids)
        {
            var check = new CheckBox { Content = id, IsChecked = selected.Contains(id) };
            panels.Children.Add(check); choices.Add(check);
        }
        var judge = Choice("Judge 模型", ids, saved ? Json.Text(profile, "judge_model") : ids[0]);
        var final = Choice("Final 模型", ids, saved ? Json.Text(profile, "final_model") : ids[0]);
        var minimum = new NumberBox { Header = "最少成功 Panel", Minimum = 1, Maximum = 8, Value = saved ? Json.Field(profile, "min_successful").GetDouble() : 1 };
        var timeout = new NumberBox { Header = "超时（毫秒）", Minimum = 1, Value = saved ? Json.Field(profile, "timeout_ms").GetDouble() : 300000 };
        var intermediate = new CheckBox { Content = "显示中间结果", IsChecked = !saved || Json.Flag(profile, "show_intermediate_results") };
        var tools = new CheckBox { Content = "允许 Panel 调用工具", IsChecked = !saved || Json.Flag(Json.Field(profile, "panel_tools"), "enabled") };
        var form = new StackPanel { Spacing = 16 };
        foreach (var element in new FrameworkElement[] { profileId, Text("Panel 模型（1–8 个）"), panels, judge, final, minimum, timeout, intermediate, tools }) form.Children.Add(element);
        var buttons = Row();
        buttons.Children.Add(Button("保存 Fusion", async () =>
        {
            var chosen = choices.Where(check => check.IsChecked == true).Select(check => (string)check.Content).ToArray();
            if (chosen.Length is < 1 or > 8 || !double.IsFinite(minimum.Value) || minimum.Value != Math.Truncate(minimum.Value) || minimum.Value < 1 || minimum.Value > chosen.Length ||
                !double.IsFinite(timeout.Value) || timeout.Value <= 0 || timeout.Value != Math.Truncate(timeout.Value))
                throw new InvalidOperationException("请选择 1–8 个 Panel，最少成功数必须为整数且不超过 Panel 数，超时必须为正整数。");
            // Preserve fields not edited by this shell; validation remains in the CLI.
            var payload = saved ? JsonSerializer.Deserialize<Dictionary<string, object?>>(profile.GetRawText())! : new Dictionary<string, object?>();
            payload["id"] = profileId.Text.Trim(); payload["panel_models"] = chosen;
            payload["judge_model"] = judge.SelectedItem; payload["final_model"] = final.SelectedItem;
            payload["min_successful"] = (int)minimum.Value; payload["timeout_ms"] = (long)timeout.Value;
            payload["show_intermediate_results"] = intermediate.IsChecked == true;
            var panelTools = saved ? JsonSerializer.Deserialize<Dictionary<string, object?>>(Json.Field(profile, "panel_tools").GetRawText())! : new Dictionary<string, object?> { ["max_rounds"] = 16, ["max_calls_per_model"] = 64 };
            panelTools["enabled"] = tools.IsChecked == true; payload["panel_tools"] = panelTools;
            if (!saved) payload["max_completion_tokens"] = 2048;
            var args = new List<string> { "fusion", "set", "--profile-json", JsonSerializer.Serialize(payload) };
            if (saved) args.AddRange(["--replace-id", Json.Text(profile, "id")]);
            await ChangeAsync("保存 Fusion", args.ToArray(), apply: true);
        }, true));
        if (saved) buttons.Children.Add(Button("删除 Fusion", async () =>
        {
            if (await ConfirmAsync("删除 Fusion？", "将移除此组合模型的配置。", "删除"))
                await ChangeAsync("删除 Fusion", ["fusion", "delete", "--id", Json.Text(profile, "id")], apply: true);
        }));
        form.Children.Add(buttons);
        PageBody.Children.Add(Card("多模型协作", "可用模型由 CLI 提供；保存后应用到已接入的客户端。", form));
    }

    private async Task SettingsAsync(CancellationToken token)
    {
        var theme = Choice("外观", ["跟随系统", "浅色", "深色"], Root.RequestedTheme switch { ElementTheme.Light => "浅色", ElementTheme.Dark => "深色", _ => "跟随系统" });
        AutomationProperties.SetAutomationId(theme, "theme-picker");
        theme.SelectionChanged += (_, _) => Root.RequestedTheme = theme.SelectedIndex switch { 1 => ElementTheme.Light, 2 => ElementTheme.Dark, _ => ElementTheme.Default };
        var startup = new CheckBox { Content = "登录 Windows 时启动界面", IsChecked = NativeShell.StartupEnabled(), IsEnabled = !NativeShell.StartupBlocked() };
        startup.Click += (_, _) =>
        {
            try { NativeShell.SetStartup(startup.IsChecked == true); }
            catch (Exception error) { startup.IsChecked = NativeShell.StartupEnabled(); ShowNotice("启动设置失败", error.Message, InfoBarSeverity.Error); }
        };
        var appearance = new StackPanel { Spacing = 16 }; appearance.Children.Add(theme); appearance.Children.Add(startup);
        appearance.Children.Add(Text(NativeShell.StartupBlocked() ? "启动项已被 Windows 禁用，请在系统「启动应用」中调整。" : "界面登录启动与网关自启分别管理。"));
        PageBody.Children.Add(Card("外观与启动", "原生主题支持系统明暗模式和高对比度。", appearance));
        var autostart = await Client.QueryAsync(["service", "autostart", "status", "--json"], token);
        var serviceButtons = Row();
        serviceButtons.Children.Add(Button("启用网关自启", () => ChangeAsync("启用网关自启", ["service", "autostart", "enable", "--json"])));
        serviceButtons.Children.Add(Button("关闭网关自启", () => ChangeAsync("关闭网关自启", ["service", "autostart", "disable", "--json"])));
        PageBody.Children.Add(Card("网关自启", Json.Flag(autostart, "enabled") ? "已启用" : "已关闭", serviceButtons));
        var ech = await Client.QueryAsync(["ech", "status", "--json"], token);
        var network = Row();
        network.Children.Add(Button("检测 ECH", () => ChangeAsync("检测 ECH", ["ech", "test", "--json"])));
        network.Children.Add(Button("启用 ECH", () => ChangeAsync("启用 ECH", ["ech", "enable"])));
        network.Children.Add(Button("关闭 ECH", () => ChangeAsync("关闭 ECH", ["ech", "disable"])));
        PageBody.Children.Add(Card("官方连接 · ECH", $"{(Json.Flag(ech, "enabled") ? "已启用" : "已关闭")} · {Json.OptionalText(ech, "fallback_reason") ?? "未发生连接回退"}", network));
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
        PageBody.Children.Add(Card("配置备份", "由 Rust CLI 导入导出，界面不会读写业务配置文件。", backup));
        var diagnostics = Row();
        diagnostics.Children.Add(Button("打开日志目录", () => OpenPathAsync(Path.GetDirectoryName(Json.Text(Json.Field(metadata, "paths"), "gateway_log"))!)));
        diagnostics.Children.Add(Button("打开配置目录", () => OpenPathAsync(Json.Text(Json.Field(metadata, "paths"), "state"))));
        diagnostics.Children.Add(Button("运行诊断", () => ChangeAsync("诊断", ["doctor", "--json"])));
        diagnostics.Children.Add(Button("重放上报", () => ChangeAsync("重放上报", ["report-replay"])));
        PageBody.Children.Add(Card("关于与诊断", $"Codex Mixin · CLI {Json.Text(metadata, "version")} · WinUI 3 / Windows App SDK", diagnostics));
    }

    private static Task OpenPathAsync(string path)
    {
        if (!Path.IsPathFullyQualified(path) || !Directory.Exists(path)) throw new DirectoryNotFoundException($"目录不存在：{path}");
        Process.Start(new ProcessStartInfo(path) { UseShellExecute = true });
        return Task.CompletedTask;
    }

    private Button Button(string label, Func<Task> work, bool primary = false)
    {
        var button = new Button { Content = label };
        if (primary) button.Style = (Style)Microsoft.UI.Xaml.Application.Current.Resources["AccentButtonStyle"];
        button.Click += async (_, _) =>
        {
            try { await work(); }
            catch (Exception error) { ShowNotice($"{label}失败", error.Message, InfoBarSeverity.Error); }
        };
        return button;
    }

    private static ActionPanel Row() => new();
    private static TextBlock Text(string value) => new() { Text = value, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
    private static TextBox Input(string header, string value) => new() { Header = header, Text = value };
    private static ComboBox Choice(string header, string[] values, string selected)
    {
        var picker = new ComboBox { Header = header, HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var value in values) picker.Items.Add(value);
        // Keep unavailable saved references visible rather than silently replacing them.
        if (!values.Contains(selected)) picker.Items.Add(selected);
        picker.SelectedItem = selected;
        return picker;
    }

    private Border Card(string title, string description, UIElement? content = null)
    {
        var body = new StackPanel { Spacing = 12 };
        body.Children.Add(new TextBlock { Text = title, Style = (Style)Microsoft.UI.Xaml.Application.Current.Resources["SubtitleTextBlockStyle"], TextWrapping = TextWrapping.Wrap });
        body.Children.Add(new TextBlock { Text = description, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true,
            Foreground = (Brush)Microsoft.UI.Xaml.Application.Current.Resources["TextFillColorSecondaryBrush"] });
        if (content != null) body.Children.Add(content);
        return new Border { Style = (Style)Microsoft.UI.Xaml.Application.Current.Resources["Card"], Child = body };
    }

    private ContentDialog Dialog(string title, UIElement content) => new()
    {
        Title = title, Content = content, CloseButtonText = "取消", XamlRoot = Root.XamlRoot,
        RequestedTheme = Root.ActualTheme, DefaultButton = ContentDialogButton.Primary
    };

    private async Task<ContentDialogResult> ShowDialogAsync(ContentDialog dialog)
    {
        if (dialogOpen) return ContentDialogResult.None;
        dialogOpen = true;
        try { return await dialog.ShowAsync(); }
        finally { dialogOpen = false; }
    }

    private async Task<bool> ConfirmAsync(string title, string message, string confirm)
    {
        var dialog = Dialog(title, Text(message)); dialog.PrimaryButtonText = confirm;
        return await ShowDialogAsync(dialog) == ContentDialogResult.Primary;
    }

    private void ShowNotice(string title, string message, InfoBarSeverity severity)
    {
        Notice.Title = title; Notice.Message = message; Notice.Severity = severity; Notice.IsOpen = true;
    }
}
