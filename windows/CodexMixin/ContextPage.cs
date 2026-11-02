using System.Text;
using System.Text.Encodings.Web;
using System.Text.Json;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.Foundation;
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
            statistics.Content = ContextStatsCards(stats, token);
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

    private UIElement ContextStatsCards(ContextStats stats, CancellationToken token)
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

        var inspector = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        inspector.Content = Card("上下文详情", "选择一个会话，查看上下文增长、压缩范围，以及原文和摘要。");
        CancellationTokenSource? archiveQuery = null;
        async Task InspectAsync(ContextSession session)
        {
            archiveQuery?.Cancel();
            var request = CancellationTokenSource.CreateLinkedTokenSource(token);
            archiveQuery = request;
            inspector.Content = Card(session.Display, "正在读取归档…", new ProgressRing { IsActive = true, Width = 24, Height = 24 });
            try
            {
                var document = await Client.QueryAsync(["context", "export", $"--session={session.Session}"], request.Token);
                request.Token.ThrowIfCancellationRequested();
                if (!ReferenceEquals(archiveQuery, request)) return;
                var detail = ContextContract.ParseArchive(document);
                if (detail.Session != session.Session) throw new CliFailure("CLI 返回了其他会话的归档。");
                inspector.Content = ContextArchiveView(session, detail);
            }
            catch (OperationCanceledException) { }
            catch (CliFailure error)
            {
                if (ReferenceEquals(archiveQuery, request) && !request.IsCancellationRequested)
                    inspector.Content = Card("归档读取失败", error.Message, Button("重试", () => InspectAsync(session)));
            }
            finally
            {
                if (ReferenceEquals(archiveQuery, request)) archiveQuery = null;
                request.Dispose();
            }
        }
        inspector.Unloaded += (_, _) => { archiveQuery?.Cancel(); };
        var archive = new StackPanel { Spacing = 8 };
        ToolTipService.SetToolTip(archive, $"归档库：{stats.DatabasePath}");
        if (stats.Sessions.Length == 0)
        {
            archive.Children.Add(Text("暂无会话。"));
        }
        else
        {
            archive.Children.Add(new Border { Child = Sessions(stats.Sessions, session => { _ = InspectAsync(session); }), Style = Themed("ListCard") });
        }

        var body = new StackPanel { Spacing = 12 };
        body.Children.Add(tiles);
        var workspace = new Grid { ColumnSpacing = 16 };
        workspace.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(0.9, GridUnitType.Star) });
        workspace.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1.1, GridUnitType.Star) });
        var sessionCard = Card("按会话统计", "", archive);
        workspace.Children.Add(sessionCard);
        Grid.SetColumn(inspector, 1); workspace.Children.Add(inspector);
        var narrowWorkspace = false;
        workspace.SizeChanged += (_, args) =>
        {
            var narrow = args.NewSize.Width < 960;
            if (narrow == narrowWorkspace) return;
            narrowWorkspace = narrow;
            workspace.RowDefinitions.Clear();
            if (narrow) { workspace.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto }); workspace.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto }); }
            Grid.SetColumn(inspector, narrow ? 0 : 1); Grid.SetRow(inspector, narrow ? 1 : 0);
            Grid.SetColumnSpan(sessionCard, narrow ? 2 : 1); Grid.SetColumnSpan(inspector, narrow ? 2 : 1);
            inspector.Margin = narrow ? new Thickness(0, 16, 0, 0) : new Thickness(0);
        };
        body.Children.Add(workspace);
        return body;
    }

    private static ListView Sessions(ContextSession[] sessions, Action<ContextSession> select)
    {
        var headers = new[] { "会话", "消息", "压缩", "节省", "视图", "命中", "更新" };
        var list = new ListView
        {
            SelectionMode = ListViewSelectionMode.Single,
            Height = Math.Min(320, 44 + sessions.Length * 52),
            Header = TableHeader(headers),
            ItemTemplate = (DataTemplate)Microsoft.UI.Xaml.Application.Current.Resources["VirtualRowTemplate"],
            ItemContainerStyle = Themed("ModelRowContainer"),
            ItemsSource = Enumerable.Range(0, sessions.Length).ToArray()
        };
        AutomationProperties.SetAutomationId(list, "context-sessions");
        list.SelectionChanged += (_, _) => { if (list.SelectedItem is int index) select(sessions[index]); };
        list.ContainerContentChanging += (_, args) =>
        {
            if (args.ItemContainer.ContentTemplateRoot is not ContentControl content) return;
            content.Content = args.InRecycleQueue ? null : Session(sessions[(int)args.Item]);
            args.Handled = true;
        };
        return list;
    }

    private UIElement ContextArchiveView(ContextSession session, ContextArchive archive)
    {
        var body = new StackPanel { Spacing = 16 };
        body.Children.Add(Text($"当前视图 {UsageView.Compact(session.ViewTokens)} token · 已节省 {UsageView.Compact(session.SavedTokens)} token"));
        body.Children.Add(new TextBlock { Text = "上下文增长", FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        var serializing = new JsonSerializerOptions { Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping };
        var messages = archive.History.ToDictionary(row => row.Reference);
        var blocks = archive.Blocks.ToDictionary(row => row.Id);
        var sizes = messages.ToDictionary(row => row.Key, row => Encoding.UTF8.GetByteCount(JsonSerializer.Serialize(row.Value.Item, serializing)));
        var visibleAt = new Dictionary<long, long>();
        foreach (var segment in archive.Segments)
        {
            var size = segment.Kind == "block" ? Encoding.UTF8.GetByteCount(blocks[segment.Reference].Summary) : sizes[segment.Reference];
            var position = segment.Kind == "block" ? segment.EndPosition : segment.Position;
            visibleAt[position] = visibleAt.GetValueOrDefault(position) + size;
        }
        long original = 0, visible = 0;
        var points = new List<(int Index, long Original, long Visible)> { (0, 0, 0) };
        var stride = Math.Max(1, (int)Math.Ceiling(archive.History.Length / 400d));
        for (var index = 0; index < archive.History.Length; index++)
        {
            var message = archive.History[index];
            original += sizes[message.Reference]; visible += visibleAt.GetValueOrDefault(message.Position);
            if ((index + 1) % stride == 0 || index == archive.History.Length - 1) points.Add((index + 1, original, visible));
        }
        var chart = new Canvas { Width = 560, Height = 196 };
        var originalLine = new Polyline { Style = Themed("ContextArchiveOriginalLine"), StrokeDashArray = new DoubleCollection { 4, 3 } };
        var visibleLine = new Polyline { Style = Themed("ContextArchiveVisibleLine") };
        var max = Math.Max(1, Math.Max(original, visible));
        foreach (var point in points)
        {
            var x = 40 + 504d * point.Index / Math.Max(1, archive.History.Length);
            originalLine.Points.Add(new Point(x, 160 - 144d * point.Original / max));
            visibleLine.Points.Add(new Point(x, 160 - 144d * point.Visible / max));
        }
        for (var index = 0; index <= 3; index++)
        {
            var y = 160 - 48d * index;
            var line = new Line { X1 = 40, X2 = 544, Y1 = y, Y2 = y, Style = Themed("ContextArchiveGridLine") };
            chart.Children.Add(line);
            var label = new TextBlock { Text = $"{max * index / 3d / 1024:0}", FontSize = 11 };
            Canvas.SetLeft(label, 0); Canvas.SetTop(label, y - 8); chart.Children.Add(label);
        }
        chart.Children.Add(originalLine); chart.Children.Add(visibleLine);
        foreach (var (x, text) in new[] { (40d, "0"), (260d, "消息顺序"), (510d, archive.History.Length.ToString()) })
        {
            var axis = new TextBlock { Text = text, FontSize = 11, Style = Themed("Caption") };
            Canvas.SetLeft(axis, x); Canvas.SetTop(axis, 174); chart.Children.Add(axis);
        }
        AutomationProperties.SetName(chart, $"归档文本增长：原始 {original / 1024d:0.0} KiB，当前视图 {visible / 1024d:0.0} KiB");
        body.Children.Add(Text("虚线：原始归档   实线：当前视图   单位：KiB"));
        body.Children.Add(new Viewbox { Child = chart, Stretch = Stretch.Uniform, HorizontalAlignment = HorizontalAlignment.Stretch });
        body.Children.Add(new TextBlock
        {
            Text = "按消息顺序累计 JSON UTF-8 体积。当前视图表示现有压缩的分布，不代表历史模型 token 用量；不含摘要包装文本。",
            Style = Themed("Caption"), TextWrapping = TextWrapping.Wrap
        });
        var segments = new StackPanel { Spacing = 4 };
        foreach (var segment in archive.Segments)
            segments.Children.Add(Text($"{(segment.Kind == "block" ? "压缩块" : "原始消息")} · {segment.Reference} · {segment.Position + 1}–{segment.EndPosition + 1}"));
        body.Children.Add(new Expander { Header = "当前上下文片段", Content = segments, HorizontalAlignment = HorizontalAlignment.Stretch });
        foreach (var block in archive.Blocks)
        {
            var detail = new StackPanel { Spacing = 12 };
            var summary = new TextBlock { Text = block.Summary, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
            detail.Children.Add(new TextBlock { Text = "压缩后的摘要", FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
            detail.Children.Add(summary);
            var sources = new Expander { Header = $"查看被压缩的原文（{block.References.Length} 条）", HorizontalAlignment = HorizontalAlignment.Stretch };
            sources.Expanding += (_, _) =>
            {
                if (sources.Content is not null) return;
                var originals = new StackPanel { Spacing = 8 };
                foreach (var reference in block.References)
                    if (messages.TryGetValue(reference, out var message)) originals.Children.Add(ContextOriginal(message));
                sources.Content = originals;
            };
            detail.Children.Add(sources);
            var title = new StackPanel { Spacing = 4 };
            title.Children.Add(Text(string.IsNullOrEmpty(block.Topic) ? block.Id : block.Topic));
            title.Children.Add(new TextBlock { Text = $"{block.Id} · T{block.Tier} · {block.StartPosition + 1}–{block.EndPosition + 1} · {(block.SupersededBy is null ? "未被替代" : $"已被 {block.SupersededBy} 替代")}", Style = Themed("Caption"), TextWrapping = TextWrapping.Wrap });
            body.Children.Add(new Expander { Header = title, Content = detail, HorizontalAlignment = HorizontalAlignment.Stretch });
        }
        if (archive.Blocks.Length == 0) body.Children.Add(Text("此会话尚无压缩块。"));
        var all = new Expander { Header = $"浏览原始归档（{archive.History.Length} 条）", HorizontalAlignment = HorizontalAlignment.Stretch };
        all.Expanding += (_, _) =>
        {
            if (all.Content is not null) return;
            var originals = new StackPanel { Spacing = 8 };
            foreach (var message in archive.History) originals.Children.Add(ContextOriginal(message));
            all.Content = originals;
        };
        body.Children.Add(all);
        return Card(session.Display, "", body);
    }

    private static Expander ContextOriginal(ContextArchiveMessage message)
    {
        var expander = new Expander { Header = message.Reference, HorizontalAlignment = HorizontalAlignment.Stretch };
        expander.Expanding += (_, _) =>
        {
            if (expander.Content is not null) return;
            expander.Content = new TextBlock
            {
                Text = JsonSerializer.Serialize(message.Item, new JsonSerializerOptions { WriteIndented = true, Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping }),
                TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true, Style = Themed("Mono")
            };
        };
        return expander;
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
