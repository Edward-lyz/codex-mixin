using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace CodexMixin;

// The 上下文 page: "context config get --json" and "context stats --json" read the
// engine snapshot, and one "context config set" call submits only the edited
// fields. The CLI owns validation, storage, and the archive database; this page
// reads and writes none of those files directly.
public sealed partial class MainWindow
{
    private static readonly double[] ContextColumns = [2.4, 0.7, 0.7, 0.9, 0.9, 0.9, 1.1];

    private async Task ContextAsync(CancellationToken token)
    {
        var settings = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        var statistics = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        PageBody.Children.Add(settings);
        PageBody.Children.Add(statistics);
        // Parse failures are contract errors, not "unknown values"; show them verbatim.
        try
        {
            var config = ContextContract.ParseConfig(await Client.QueryAsync(["context", "config", "get", "--json"], token));
            settings.Content = ContextSettingsCard(config.Settings);
        }
        catch (CliFailure error)
        {
            settings.Content = Card("上下文设置不可用", error.Message);
        }
        try
        {
            var stats = ContextContract.ParseStats(await Client.QueryAsync(["context", "stats", "--json"], token));
            statistics.Content = ContextStatsCards(stats);
        }
        catch (CliFailure error)
        {
            statistics.Content = Card("上下文统计不可用", error.Message);
        }
    }

    private Border ContextSettingsCard(ContextSettings stored)
    {
        var enabled = new ToggleSwitch
        {
            Header = "上下文引擎", OnContent = "已启用", OffContent = "已停用", IsOn = stored.Enabled
        };
        AutomationProperties.SetAutomationId(enabled, "context-enabled");
        var growth = Amount("增长阈值（tokens）", stored.GrowthThresholdTokens, "context-growth-threshold");
        var window = Amount("上下文窗口（tokens）", stored.ContextWindowTokens, "context-window-tokens");
        window.PlaceholderText = "留空表示跟随模型";
        var pressure = Fraction("压力比例", stored.PressureRatio, "context-pressure-ratio");
        var urgent = Fraction("紧急比例", stored.UrgentRatio, "context-urgent-ratio");
        var protectedMessages = Amount("保护消息数", stored.ProtectedMessages, "context-protected-messages");
        var protectedTokens = Amount("保护 token 数", stored.ProtectedTokens, "context-protected-tokens");
        var rounds = Amount("内部续轮上限", stored.MaxInternalRounds, "context-max-internal-rounds");

        var fields = new Grid { ColumnSpacing = 12, RowSpacing = 12 };
        for (var column = 0; column < 2; column++)
            fields.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        var cells = new FrameworkElement[] { growth, window, pressure, urgent, protectedMessages, protectedTokens, rounds };
        for (var index = 0; index < cells.Length; index++)
        {
            Grid.SetRow(cells[index], index / 2);
            Grid.SetColumn(cells[index], index % 2);
            fields.Children.Add(cells[index]);
        }
        fields.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        fields.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        fields.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        fields.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });

        async Task SaveAsync()
        {
            var edited = stored with
            {
                Enabled = enabled.IsOn,
                GrowthThresholdTokens = Whole(growth, "增长阈值"),
                PressureRatio = FractionValue(pressure, "压力比例"),
                UrgentRatio = FractionValue(urgent, "紧急比例"),
                ProtectedMessages = Whole(protectedMessages, "保护消息数"),
                ProtectedTokens = Whole(protectedTokens, "保护 token 数"),
                MaxInternalRounds = Whole(rounds, "内部续轮上限"),
                ContextWindowTokens = WindowTokens(window)
            };
            if (ContextContract.SaveArguments(stored, edited) is not { } arguments)
            {
                ShowNotice("保存设置", "没有需要保存的更改。", InfoBarSeverity.Informational);
                return;
            }
            await ChangeAsync("保存设置", arguments);
        }

        var actions = Row();
        var save = Button("保存设置", SaveAsync, primary: true);
        AutomationProperties.SetAutomationId(save, "context-save");
        var restart = Button("重启网关", () => ChangeAsync("重启网关", ["service", "restart", "--managed", "--json"]));
        AutomationProperties.SetAutomationId(restart, "context-restart");
        actions.Children.Add(save);
        actions.Children.Add(restart);

        var body = new StackPanel { Spacing = 12 };
        body.Children.Add(enabled);
        body.Children.Add(fields);
        body.Children.Add(actions);
        body.Children.Add(Text("保存时只提交改动过的字段；开关与阈值只在网关重启后生效。"));
        return Card("上下文引擎", "增长阈值决定何时开始压缩，压力与紧急比例决定压缩时机，保护消息与 token 不进入压缩。", body);
    }

    private UIElement ContextStatsCards(ContextStats stats)
    {
        var totals = stats.Totals;
        var cache = totals.Cache;
        var tiles = new Grid { ColumnSpacing = 12 };
        var metrics = new (string Title, string Value, string Note)[]
        {
            ("会话", UsageView.Compact(totals.Sessions), $"{UsageView.Compact(totals.Messages)} 条归档消息"),
            ("压缩次数", UsageView.Compact(totals.Compressions), "含已被更高层合并的压缩"),
            ("节省 tokens", UsageView.Compact(totals.SavedTokens), $"当前视图 {UsageView.Compact(totals.ViewTokens)}"),
            ("缓存命中", cache.HitPercent is null ? "—" : $"{cache.HitPercent:0.0}%",
                cache.HitPercent is null ? "缓存命中率不可用" : "仅统计支持缓存观测的请求")
        };
        for (var index = 0; index < metrics.Length; index++)
        {
            tiles.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var (title, value, note) = metrics[index];
            var tileBody = new StackPanel { Spacing = 2 };
            tileBody.Children.Add(new TextBlock { Text = value, FontSize = 24, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
            tileBody.Children.Add(new TextBlock { Text = note, Style = Themed("Caption") });
            var tile = Card(title, "", tileBody);
            Grid.SetColumn(tile, index);
            tiles.Children.Add(tile);
        }

        var archive = new StackPanel { Spacing = 8 };
        var path = Text(stats.DatabasePath);
        path.Style = Themed("Mono");
        ToolTipService.SetToolTip(path, $"归档库：{stats.DatabasePath}");
        archive.Children.Add(path);
        if (stats.Sessions.Length == 0)
        {
            archive.Children.Add(Text("归档库中还没有上下文会话；新会话第一次压缩后才会出现。"));
        }
        else
        {
            archive.Children.Add(new Border { Child = Sessions(stats.Sessions), Style = Themed("ListCard") });
        }

        var body = new StackPanel { Spacing = 12 };
        body.Children.Add(tiles);
        body.Children.Add(Card("按会话统计", "压缩次数含已被更高层合并的压缩，节省 tokens 只统计当前视图仍生效的压缩块。", archive));
        return body;
    }

    private static ListView Sessions(ContextSession[] sessions)
    {
        var headers = new[] { "会话", "消息", "压缩", "节省", "视图", "命中", "更新" };
        var list = new ListView
        {
            SelectionMode = ListViewSelectionMode.None, Height = Math.Min(320, 44 + sessions.Length * 52),
            Header = TableHeader(headers),
            ItemTemplate = (DataTemplate)Microsoft.UI.Xaml.Application.Current.Resources["VirtualRowTemplate"],
            ItemContainerStyle = Themed("ModelRowContainer"),
            ItemsSource = Enumerable.Range(0, sessions.Length).ToArray()
        };
        AutomationProperties.SetAutomationId(list, "context-sessions");
        list.ContainerContentChanging += (_, args) =>
        {
            if (args.ItemContainer.ContentTemplateRoot is not ContentControl content) return;
            content.Content = args.InRecycleQueue ? null : Session(sessions[(int)args.Item]);
            args.Handled = true;
        };
        return list;
    }

    private static Border Session(ContextSession session)
    {
        var display = new TextBlock { Text = session.Display.Trim(), TextTrimming = TextTrimming.CharacterEllipsis };
        ToolTipService.SetToolTip(display, $"{session.Session}\nrevision {session.Revision}");
        var updated = session.UpdatedAt <= 0 ? "未知"
            : DateTimeOffset.FromUnixTimeSeconds(session.UpdatedAt).ToLocalTime().ToString("MM-dd HH:mm");
        var hit = session.Cache.HitPercent is null ? "—" : $"{session.Cache.HitPercent:0.0}%";
        var cells = new UIElement[]
        {
            display,
            new TextBlock { Text = UsageView.Compact(session.Messages) },
            new TextBlock { Text = UsageView.Compact(session.Compressions) },
            new TextBlock { Text = UsageView.Compact(session.SavedTokens) },
            new TextBlock { Text = UsageView.Compact(session.ViewTokens) },
            new TextBlock { Text = hit },
            new TextBlock { Text = updated, Style = Themed("Mono") }
        };
        return TableRow(cells, true);
    }

    private static Border TableHeader(string[] labels)
    {
        var cells = labels.Select(label => (UIElement)new TextBlock { Text = label, Style = Themed("Caption") }).ToArray();
        return TableRow(cells, false);
    }

    private static Border TableRow(UIElement[] cells, bool divider)
    {
        var grid = new Grid { ColumnSpacing = 10 };
        for (var index = 0; index < cells.Length; index++)
        {
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(ContextColumns[index], GridUnitType.Star) });
            if (cells[index] is FrameworkElement element) element.VerticalAlignment = VerticalAlignment.Center;
            Grid.SetColumn((FrameworkElement)cells[index], index);
            grid.Children.Add(cells[index]);
        }
        return new Border
        {
            Child = grid, Padding = new Thickness(16, 8, 16, 8), Style = Themed(divider ? "RowDivider" : "RowPlain"),
            BorderThickness = divider ? new Thickness(0, 1, 0, 0) : new Thickness(0)
        };
    }

    private static NumberBox Amount(string header, long value, string id)
    {
        var box = new NumberBox { Header = header, Value = value, MinWidth = 180 };
        AutomationProperties.SetAutomationId(box, id);
        AutomationProperties.SetName(box, header);
        return box;
    }

    private static NumberBox Amount(string header, long? value, string id)
    {
        var box = Amount(header, 0, id);
        box.Value = value ?? double.NaN;
        return box;
    }

    private static NumberBox Fraction(string header, double value, string id)
    {
        var box = new NumberBox { Header = header, Value = value, MinWidth = 180 };
        AutomationProperties.SetAutomationId(box, id);
        AutomationProperties.SetName(box, header);
        return box;
    }

    // NumberBox holds a double; integers are still validated here so a fractional
    // entry never silently reaches the CLI. Range checks stay in the CLI.
    private static long Whole(NumberBox box, string label)
    {
        var value = box.Value;
        return double.IsFinite(value) && value == Math.Truncate(value) && Math.Abs(value) <= long.MaxValue ? (long)value
            : throw new InvalidOperationException($"{label}必须是整数。");
    }

    private static double FractionValue(NumberBox box, string label) =>
        double.IsFinite(box.Value) ? box.Value : throw new InvalidOperationException($"{label}必须是数字。");

    private static long? WindowTokens(NumberBox box) => double.IsNaN(box.Value) ? null : Whole(box, "上下文窗口 token 数");
}
