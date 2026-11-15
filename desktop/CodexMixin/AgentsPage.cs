using System.Text.Json;
using Avalonia;
using Avalonia.Automation;
using Avalonia.Controls;
using Avalonia.Layout;
using Avalonia.Media;
using Arc = TKCode.Arc;

namespace CodexMixin;

internal sealed partial class MainWindow
{
    private async Task<Control> BuildAgentsAsync(CancellationToken token)
    {
        var agents = Json.Rows(Json.Field(await cli.QueryAsync(["agent", "list", "--json"], token), "agents"));
        var page = new StackPanel { Spacing = 12, MaxWidth = 880, HorizontalAlignment = HorizontalAlignment.Left };

        var connectedCount = agents.Count(agent => Json.Text(Json.Field(agent, "integration"), "state") is "managed" or "drift");
        var detectedCount = agents.Count(agent => Json.Flag(agent, "detected"));
        var toolbar = new Grid { ColumnDefinitions = new ColumnDefinitions("*,Auto") };
        var summary = new TextBlock
        {
            Text = Localized.Format("已接入 {0} 个 Agent · 已检测到 {1} 个 · {2:HH:mm:ss}", connectedCount, detectedCount, DateTimeOffset.Now),
            VerticalAlignment = VerticalAlignment.Center,
            Opacity = 0.7
        };
        AutomationProperties.SetAutomationId(summary, "agent-summary");
        toolbar.Children.Add(summary);
        var check = new Arc.Button { Content = Localized.Get("检查版本更新"), Variant = Arc.ButtonVariant.Secondary };
        AutomationProperties.SetAutomationId(check, "agent-check-versions");
        Grid.SetColumn(check, 1);
        toolbar.Children.Add(check);
        page.Children.Add(toolbar);

        var results = new StackPanel { Spacing = 8 };
        page.Children.Add(results);
        check.Click += async (_, _) =>
        {
            check.IsLoading = true;
            try { await ShowAgentVersionsAsync(results, token); }
            finally { check.IsLoading = false; }
        };

        var detected = new StackPanel { Spacing = 8 };
        var undetected = new StackPanel { Spacing = 8 };
        foreach (var agent in agents)
        {
            var row = AgentRow(agent);
            var connected = Json.Text(Json.Field(agent, "integration"), "state") is "managed" or "drift" or "pending" or "pending_removal";
            (connected || Json.Flag(agent, "detected") ? detected : undetected).Children.Add(row);
        }
        if (detected.Children.Count > 0) page.Children.Add(detected);
        if (undetected.Children.Count > 0)
        {
            var hidden = new Arc.ExpandableCard
            {
                Header = Localized.Format("显示未接入且未检测到的 {0} 个 Agent", undetected.Children.Count),
                Content = undetected
            };
            AutomationProperties.SetAutomationId(hidden, "agent-undetected");
            page.Children.Add(hidden);
        }
        if (agents.Length == 0)
            page.Children.Add(new Arc.EmptyState { Title = Localized.Get("Agent 接入管理"), Description = Localized.Get("CLI 未返回任何 Agent。") });
        return page;
    }

    // Extra query: failures are shown as an alert on the page.
    private async Task ShowAgentVersionsAsync(StackPanel results, CancellationToken token)
    {
        results.Children.Clear();
        try
        {
            var versions = Json.Rows(Json.Field(await cli.QueryAsync(["agent", "versions", "--json", "--refresh"], token), "agents"));
            var body = new StackPanel { Spacing = 8 };
            foreach (var agent in versions)
            {
                var version = Json.Field(agent, "cli");
                var id = Json.Text(agent, "id");
                var line = new Grid { ColumnDefinitions = new ColumnDefinitions("*,Auto") };
                line.Children.Add(new TextBlock
                {
                    Text = Localized.Format("{0} · 当前 {1} · 最新 {2}", id,
                        Json.OptionalText(version, "version") ?? Json.OptionalText(version, "app_version") ?? Localized.Get("未知"),
                        Json.OptionalText(version, "latest") ?? Localized.Get("未知")) +
                        (Json.OptionalText(version, "error") is { } error ? $" · {error}" : ""),
                    TextWrapping = TextWrapping.Wrap,
                    VerticalAlignment = VerticalAlignment.Center
                });
                if (Json.Flag(version, "update"))
                {
                    var upgrade = new Arc.Button { Content = Localized.Get("升级"), Size = Arc.ControlSize.Small };
                    upgrade.Click += async (_, _) => await RunActionAsync(["agent", "upgrade", id, "--json"]);
                    Grid.SetColumn(upgrade, 1); line.Children.Add(upgrade);
                }
                body.Children.Add(line);
            }
            results.Children.Add(new Arc.Alert { Title = Localized.Get("版本检查结果"), Tone = Arc.AlertTone.Info, Content = body });
        }
        catch (Exception error) when (error is CliFailure or JsonException or OperationCanceledException)
        {
            results.Children.Add(new Arc.Alert { Title = Localized.Get("检查 Agent 版本"), Tone = Arc.AlertTone.Danger, Content = error.Message });
        }
    }

    private Control AgentRow(JsonElement agent)
    {
        var id = Json.Text(agent, "id");
        var integration = Json.Field(agent, "integration");
        var state = Json.Text(integration, "state");
        var version = Json.Field(agent, "cli");
        var mode = Json.OptionalText(integration, "mode");
        var stateLabel = state switch
        {
            "managed" => Localized.Get("已接入"),
            "unmanaged" => Localized.Get("未接入"),
            "drift" => Localized.Get("配置已改变"),
            "error" => Localized.Get("检测失败"),
            "unsupported" => Localized.Get("暂不支持接入"),
            "pending" => Localized.Get("等待原生应用确认"),
            "pending_removal" => Localized.Get("等待原生应用移除"),
            _ => Localized.Format("未知状态：{0}", state)
        };
        if (mode == "native_models" && state == "managed") stateLabel = Localized.Get("原生模型已托管");
        var modeLabel = mode switch
        {
            "native_models" => Localized.Get("厂商原生模型管理"),
            "native_credentials" => Localized.Get("原生凭据存储"),
            "native_runtime" => Localized.Get("原生运行时"),
            "native_app" => Localized.Get("原生应用 API"),
            "confirmed_import" => Localized.Get("应用内确认导入"),
            _ => ""
        };
        if (modeLabel.Length > 0) stateLabel += $" · {modeLabel}";
        var connected = state is "managed" or "drift" or "pending" or "pending_removal";
        if (connected && !Json.Flag(agent, "detected")) stateLabel += " · " + Localized.Get("未检测到安装程序");

        var cliVersion = Json.OptionalText(version, "version");
        var appVersion = Json.OptionalText(version, "app_version");
        var versions = string.Join("  ", new[]
        {
            cliVersion is null ? null : appVersion != null ? $"CLI {cliVersion}" : cliVersion,
            appVersion is null ? null : $"App {appVersion}"
        }.Where(value => value != null));
        var header = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        header.Children.Add(new TextBlock { Text = Json.Text(agent, "name"), VerticalAlignment = VerticalAlignment.Center });
        header.Children.Add(new Arc.Badge
        {
            Content = connected ? Localized.Get("已接入") : state == "error" ? Localized.Get("检测失败") : Localized.Get("未接入"),
            Size = Arc.BadgeSize.Small,
            Tone = connected ? Arc.BadgeTone.Success : state == "error" ? Arc.BadgeTone.Warning : Arc.BadgeTone.Neutral
        });
        if (versions.Length > 0) header.Children.Add(new TextBlock { Text = versions, VerticalAlignment = VerticalAlignment.Center, Opacity = 0.6 });

        var body = new StackPanel { Spacing = 12 };
        var problem = Json.OptionalText(integration, "error") ?? (Json.OptionalText(version, "error") is { } failure ? Localized.Format("版本检查失败：{0}", failure) : null);
        body.Children.Add(new TextBlock { Text = problem ?? Json.Text(integration, "detail"), TextWrapping = TextWrapping.Wrap, Opacity = 0.7 });

        Arc.Select? codexMode = null;
        if (id == "codex" && !connected)
        {
            codexMode = new Arc.Select { Label = Localized.Get("接入模式"), HorizontalAlignment = HorizontalAlignment.Stretch };
            codexMode.Items.Add(new Arc.SelectItem { Content = Localized.Get("官方账号 + 自定义模型"), Value = "official-account" });
            codexMode.Items.Add(new Arc.SelectItem { Content = Localized.Get("仅自定义模型"), Value = "custom-only" });
            codexMode.SelectedValue = "official-account";
            AutomationProperties.SetAutomationId(codexMode, "agent-codex-mode");
            body.Children.Add(codexMode);
        }

        var controls = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
        var connect = new Arc.Switch { Content = Localized.Format("{0} 接入", Json.Text(agent, "name")), IsChecked = connected, IsEnabled = state != "unsupported" };
        AutomationProperties.SetAutomationId(connect, "agent-switch-" + id);
        var restoring = false;
        connect.IsCheckedChanged += async (_, _) =>
        {
            if (restoring) return;
            var wantConnect = connect.IsChecked == true;
            // The page reloads from the CLI after the action, so the switch always returns to the saved state first.
            restoring = true; connect.IsChecked = connected; restoring = false;
            if (!wantConnect)
            {
                if (await ConfirmAsync(Localized.Get("断开 Agent？"), Localized.Get("CLI 将恢复或移除此 Agent 的托管配置。")))
                    await RunActionAsync(["agent", "disconnect", id]);
                return;
            }
            var args = new List<string> { "agent", "connect", id };
            if (codexMode != null) args.AddRange(["--mode", codexMode.SelectedValue as string ?? "official-account"]);
            await RunActionAsync(args.ToArray());
        };
        controls.Children.Add(connect);
        if (Json.Flag(version, "update"))
        {
            var update = new Arc.Button { Content = Localized.Format("更新到 {0}", Json.Text(version, "latest")), Variant = Arc.ButtonVariant.Secondary };
            update.Click += async (_, _) => await RunActionAsync(["agent", "upgrade", id, "--json"]);
            controls.Children.Add(update);
        }
        body.Children.Add(controls);

        var card = new Arc.ExpandableCard { Header = header, Description = stateLabel, Content = body };
        AutomationProperties.SetAutomationId(card, "agent-card-" + id);
        return card;
    }
}
