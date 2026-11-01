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
        var enabled = new CheckBox
        {
            Content = "启用上下文引擎",
            IsChecked = stored.Enabled
        };
        AutomationProperties.SetAutomationId(enabled, "context-enabled");

        async Task SaveAsync()
        {
            var edited = stored with
            {
                Enabled = enabled.IsChecked == true
            };
            if (ContextContract.SaveArguments(stored, edited) is not { } arguments)
            {
                draftDirty = false;
                ApplyChanges.IsEnabled = false;
                ShowSummary("未修改");
                ShowNotice("保存设置", "没有需要保存的更改。", InfoBarSeverity.Informational);
                return;
            }
            await ChangeAsync("保存并应用设置", arguments);
        }

        pendingApply = SaveAsync;
        ApplyChanges.Content = "保存设置";
        ToolTipService.SetToolTip(ApplyChanges, "保存更改后自动重启运行中的网关并生效。");
        TrackDraft(enabled);

        var body = new StackPanel { Spacing = 12 };
        body.Children.Add(enabled);
        return Card("上下文引擎", "", body);
    }

    private UIElement ContextStatsCards(ContextStats stats)
    {
        var totals = stats.Totals;
        var cache = totals.Cache;
        var tiles = new Grid { ColumnSpacing = 12 };
        var metrics = new (string Title, string Value, string Note)[]
        {
            ("会话", UsageView.Compact(totals.Sessions), $"{UsageView.Compact(totals.Messages)} 条归档消息"),
            ("压缩次数", UsageView.Compact(totals.Compressions), ""),
            ("节省 tokens", UsageView.Compact(totals.SavedTokens), $"当前视图 {UsageView.Compact(totals.ViewTokens)}"),
            ("缓存命中", cache.HitPercent is null ? "—" : $"{cache.HitPercent:0.0}%",
                "")
        };
        for (var index = 0; index < metrics.Length; index++)
        {
            tiles.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var (title, value, note) = metrics[index];
            var tileBody = new StackPanel { Spacing = 2 };
            tileBody.Children.Add(new TextBlock { Text = value, FontSize = 24, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
            if (!string.IsNullOrWhiteSpace(note))
                tileBody.Children.Add(new TextBlock { Text = note, Style = Themed("Caption") });
            var tile = Card(title, "", tileBody);
            Grid.SetColumn(tile, index);
            tiles.Children.Add(tile);
        }

        var archive = new StackPanel { Spacing = 8 };
        ToolTipService.SetToolTip(archive, $"归档库：{stats.DatabasePath}");
        if (stats.Sessions.Length == 0)
        {
            archive.Children.Add(Text("暂无会话。"));
        }
        else
        {
            archive.Children.Add(new Border { Child = Sessions(stats.Sessions), Style = Themed("ListCard") });
        }

        var body = new StackPanel { Spacing = 12 };
        body.Children.Add(tiles);
        body.Children.Add(Card("按会话统计", "", archive));
        return body;
    }

    private static ListView Sessions(ContextSession[] sessions)
    {
        var headers = new[] { "会话", "消息", "压缩", "节省", "视图", "命中", "更新" };
        var list = new ListView
        {
            SelectionMode = ListViewSelectionMode.None,
            Height = Math.Min(320, 44 + sessions.Length * 52),
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
            Child = grid,
            Padding = new Thickness(16, 8, 16, 8),
            Style = Themed(divider ? "RowDivider" : "RowPlain"),
            BorderThickness = divider ? new Thickness(0, 1, 0, 0) : new Thickness(0)
        };
    }
}
