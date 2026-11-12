using System.Text;
using System.Windows.Media;
using System.Windows.Shapes;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;

namespace CodexMixin;

// The 上下文 page: "context config get --json" and "context stats --json" read the
// engine snapshot, and one "context config set" call submits only the edited
// fields. The CLI owns validation, storage, and the archive database; this page
// reads and writes none of those files directly.
public sealed partial class MainWindow
{
    private static readonly double[] ContextColumns = [3, 0.8, 1.2];

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
            settings.Content = Card(Localized.Get("上下文设置不可用"), error.Message);
        }
        try
        {
            var stats = ContextContract.ParseStats(await Client.QueryAsync(["context", "stats", "--json"], token));
            statistics.Content = ContextStatsCards(stats, token);
        }
        catch (CliFailure error)
        {
            statistics.Content = Card(Localized.Get("上下文统计不可用"), error.Message);
        }
    }

    private Border ContextSettingsCard(ContextSettings stored)
    {
        var enabled = new CheckBox
        {
            Content = Localized.Get("启用上下文引擎"),
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
                ShowSummary(Localized.Get("未修改"));
                ShowNotice(Localized.Get("保存设置"), Localized.Get("没有需要保存的更改。"), InfoBarSeverity.Informational);
                return;
            }
            await ChangeAsync(Localized.Get("保存并应用设置"), arguments);
        }

        pendingApply = SaveAsync;
        ApplyChanges.Content = Localized.Get("保存设置");
        ToolTipService.SetToolTip(ApplyChanges, Localized.Get("保存更改后自动重启运行中的网关并生效。"));
        TrackDraft(enabled);

        var body = new Stack { Spacing = 12 };
        body.Children.Add(enabled);
        return Card(Localized.Get("上下文引擎"), "", body);
    }

    private UIElement ContextStatsCards(ContextStats stats, CancellationToken token)
    {
        var totals = stats.Totals;
        var cache = totals.Cache;
        var tiles = new SpacedGrid { ColumnSpacing = 12 };
        var metrics = new (string Title, string Value, string Note)[]
        {
            (Localized.Get("会话"), UsageView.Compact(totals.Sessions), Localized.Format("{0} 条归档消息", UsageView.Compact(totals.Messages))),
            (Localized.Get("压缩次数"), UsageView.Compact(totals.Compressions), ""),
            (Localized.Get("节省 tokens"), UsageView.Compact(totals.SavedTokens), Localized.Format("当前视图 {0}", UsageView.Compact(totals.ViewTokens))),
            (Localized.Get("缓存命中"), cache.HitPercent is null ? "—" : $"{cache.HitPercent:0.0}%",
                "")
        };
        for (var index = 0; index < metrics.Length; index++)
        {
            tiles.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var (title, value, note) = metrics[index];
            var tileBody = new Stack { Spacing = 2 };
            tileBody.Children.Add(new TextBlock { Text = value, FontSize = 24, FontWeight = FontWeights.SemiBold });
            if (!string.IsNullOrWhiteSpace(note))
                tileBody.Children.Add(new TextBlock { Text = note, Style = Themed("Caption") });
            var tile = Card(title, "", tileBody);
            Grid.SetColumn(tile, index);
            tiles.Children.Add(tile);
        }

        var inspector = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        inspector.Content = Card(Localized.Get("上下文详情"), Localized.Get("选择一个会话，查看上下文增长，以及消息和工具如何合并成精简摘要。"));
        CancellationTokenSource? archiveQuery = null;
        async Task InspectAsync(ContextSession session)
        {
            archiveQuery?.Cancel();
            var request = CancellationTokenSource.CreateLinkedTokenSource(token);
            archiveQuery = request;
            inspector.Content = Card(session.Display, Localized.Get("正在读取归档…"), new ProgressRing { IsActive = true, Width = 24, Height = 24 });
            try
            {
                var document = await Client.QueryAsync(["context", "export", "--compact", $"--session={session.Session}"], request.Token);
                request.Token.ThrowIfCancellationRequested();
                if (!ReferenceEquals(archiveQuery, request)) return;
                var detail = ContextContract.ParseArchive(document);
                if (detail.Session != session.Session) throw new CliFailure(Localized.Get("CLI 返回了其他会话的归档。"));
                inspector.Content = ContextArchiveView(session, detail);
            }
            catch (OperationCanceledException) { }
            catch (CliFailure error)
            {
                if (ReferenceEquals(archiveQuery, request) && !request.IsCancellationRequested)
                    inspector.Content = Card(Localized.Get("归档读取失败"), error.Message, Button(Localized.Get("重试"), () => InspectAsync(session)));
            }
            finally
            {
                if (ReferenceEquals(archiveQuery, request)) archiveQuery = null;
                request.Dispose();
            }
        }
        inspector.Unloaded += (_, _) => { archiveQuery?.Cancel(); };
        var archive = new Stack { Spacing = 8 };
        ToolTipService.SetToolTip(archive, Localized.Format("归档库：{0}", stats.DatabasePath));
        if (stats.Sessions.Length == 0)
        {
            archive.Children.Add(Text(Localized.Get("暂无会话。")));
        }
        else
        {
            var sessionTable = new Stack();
            sessionTable.Children.Add(TableHeader(new[] { "会话", "压缩", "节省" }.Select(Localized.Get).ToArray()));
            sessionTable.Children.Add(Sessions(stats.Sessions, session => { _ = InspectAsync(session); }));
            archive.Children.Add(new Border { Child = sessionTable, Style = Themed("ListCard") });
        }

        var body = new Stack { Spacing = 12 };
        body.Children.Add(tiles);
        var workspace = new SpacedGrid { ColumnSpacing = 16 };
        workspace.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(0.9, GridUnitType.Star) });
        workspace.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1.1, GridUnitType.Star) });
        var sessionCard = Card(Localized.Get("按会话统计"), "", archive);
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

    private static VirtualList Sessions(ContextSession[] sessions, Action<ContextSession> select)
    {
        var list = new VirtualList
        {
            SelectionMode = SelectionMode.Single,
            Height = Math.Min(276, sessions.Length * 52 + 4),
            ItemContainerStyle = Themed("ModelRowContainer"),
            ItemsSource = Enumerable.Range(0, sessions.Length).ToArray()
        };
        AutomationProperties.SetAutomationId(list, "context-sessions");
        list.SelectionChanged += (_, _) => { if (list.SelectedItem is int index) select(sessions[index]); };
        list.Prepared += (container, item) => container.Content = Session(sessions[(int)item]);
        return list;
    }

    private UIElement ContextArchiveView(ContextSession session, ContextArchive archive)
    {
        var body = new Stack { Spacing = 16 };
        body.Children.Add(Text(Localized.Format("当前视图 {0} token · 已节省 {1} token", UsageView.Compact(session.ViewTokens), UsageView.Compact(session.SavedTokens))));
        body.Children.Add(new TextBlock { Text = Localized.Get("上下文增长"), FontWeight = FontWeights.SemiBold });
        var messages = archive.History.ToDictionary(row => row.Reference);
        var blocks = archive.Blocks.ToDictionary(row => row.Id);
        var sizes = messages.ToDictionary(row => row.Key, row => row.Value.Bytes);
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
        foreach (var (x, text) in new[] { (40d, "0"), (260d, Localized.Get("消息顺序")), (510d, archive.History.Length.ToString()) })
        {
            var axis = new TextBlock { Text = text, FontSize = 11, Style = Themed("Caption") };
            Canvas.SetLeft(axis, x); Canvas.SetTop(axis, 174); chart.Children.Add(axis);
        }
        AutomationProperties.SetName(chart, Localized.Format("归档文本增长：原始 {0:0.0} KiB，当前视图 {1:0.0} KiB", original / 1024d, visible / 1024d));
        body.Children.Add(Text(Localized.Get("虚线：原始归档   实线：当前视图   单位：KiB")));
        body.Children.Add(new Viewbox { Child = chart, Stretch = Stretch.Uniform, HorizontalAlignment = HorizontalAlignment.Stretch });
        body.Children.Add(new TextBlock
        {
            Text = Localized.Get("按消息顺序累计 JSON UTF-8 体积。当前视图表示现有压缩的分布，不代表历史模型 token 用量；不含摘要包装文本。"),
            Style = Themed("Caption"), TextWrapping = TextWrapping.Wrap
        });
        body.Children.Add(new TextBlock { Text = Localized.Get("压缩概览"), FontWeight = FontWeights.SemiBold });
        foreach (var block in archive.Blocks.Where(block => block.SupersededBy is null))
            body.Children.Add(ContextCompression(block, messages));
        if (archive.Blocks.Length == 0) body.Children.Add(Text(Localized.Get("此会话尚无压缩块。")));
        var remaining = archive.Segments.Count(segment => segment.Kind == "message");
        if (remaining > 0) body.Children.Add(new TextBlock { Text = Localized.Format("尚未压缩 · {0} 条内容", remaining), Style = Themed("Caption") });
        var earlier = archive.Blocks.Where(block => block.SupersededBy is not null).ToArray();
        if (earlier.Length > 0)
        {
            var history = new Expander { Header = Localized.Format("历史压缩（{0}）", earlier.Length), HorizontalAlignment = HorizontalAlignment.Stretch };
            history.Expanded += (_, _) =>
            {
                if (history.Content is not null) return;
                var cards = new Stack { Spacing = 12 };
                foreach (var block in earlier) cards.Children.Add(ContextCompression(block, messages));
                history.Content = cards;
            };
            body.Children.Add(history);
        }
        return Card(session.Display, "", body);
    }

    private static Border ContextCompression(ContextArchiveBlock block, Dictionary<string, ContextArchiveMessage> messages)
    {
        var sources = new Stack { Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
        var covered = block.References.Select(reference => messages.GetValueOrDefault(reference)
            ?? new ContextArchiveMessage(reference, 0, 0, "other", null));
        var groups = covered.GroupBy(message => (message.Kind, message.Tool))
            .OrderByDescending(group => group.Count()).ThenBy(group => group.Key.Kind).ThenBy(group => group.Key.Tool).ToArray();
        foreach (var group in groups.Take(5))
        {
            var (kind, tool) = group.Key;
            var label = tool?.Split('.').Last().Split("__").Last() ?? kind switch
            {
                "user" => Localized.Get("用户消息"), "assistant" => Localized.Get("助手消息"), "function_call" => Localized.Get("工具调用"),
                "function_call_output" => Localized.Get("工具结果"), "reasoning" => Localized.Get("推理"), _ => Localized.Get("其他")
            };
            var glyph = kind switch { "user" => "\uE77B", "assistant" => "\uE8BD",
                "function_call" => "\uE713", "function_call_output" => "\uE8A5", _ => "\uE712" };
            var chip = new SpacedGrid { ColumnSpacing = 6 };
            chip.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(16) });
            chip.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            chip.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var icon = new FontIcon { Glyph = glyph, FontSize = 14 };
            chip.Children.Add(icon);
            var title = new TextBlock { Text = label, Style = Themed("Caption"), TextTrimming = TextTrimming.CharacterEllipsis };
            Grid.SetColumn(title, 1); chip.Children.Add(title);
            var count = new TextBlock { Text = $"×{group.Count()}", Style = Themed("Caption") };
            Grid.SetColumn(count, 2); chip.Children.Add(count);
            ToolTipService.SetToolTip(chip, tool ?? label);
            AutomationProperties.SetName(chip, Localized.Format("{0}，{1} 条", label, group.Count()));
            sources.Children.Add(chip);
        }
        if (groups.Length > 5) sources.Children.Add(new TextBlock { Text = Localized.Format("其他 {0} 条", groups.Skip(5).Sum(group => group.Count())), Style = Themed("Caption") });
        var flow = new SpacedGrid { ColumnSpacing = 10 };
        flow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(124) });
        flow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(24) });
        flow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        flow.Children.Add(sources);
        var brace = new TextBlock { Text = "}", FontSize = 72, FontWeight = FontWeights.ExtraLight,
            Style = Themed("Caption"), VerticalAlignment = VerticalAlignment.Center };
        Grid.SetColumn(brace, 1); flow.Children.Add(brace);
        var output = new Stack { Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
        output.Children.Add(new TextBlock { Text = string.IsNullOrEmpty(block.Topic) ? Localized.Get("精简摘要") : block.Topic,
            TextWrapping = TextWrapping.Wrap, MaxHeight = 40, TextTrimming = TextTrimming.CharacterEllipsis,
            FontWeight = FontWeights.SemiBold });
        output.Children.Add(new TextBlock { Text = Localized.Format("{0} 条内容 → 1 条摘要", block.References.Length), Style = Themed("Caption"), TextWrapping = TextWrapping.Wrap });
        output.Children.Add(new TextBlock { Text = block.Summary[..Math.Min(240, block.Summary.Length)],
            Style = Themed("Caption"), TextWrapping = TextWrapping.Wrap, MaxHeight = 52, TextTrimming = TextTrimming.CharacterEllipsis });
        Grid.SetColumn(output, 2); flow.Children.Add(output);
        var detail = new Expander { Header = Localized.Get("查看摘要"), HorizontalAlignment = HorizontalAlignment.Stretch };
        detail.Expanded += (_, _) =>
        {
            if (detail.Content is not null) return;
            detail.Content = new TextBox { Text = block.Summary, TextWrapping = TextWrapping.Wrap, IsReadOnly = true, BorderThickness = new Thickness(0), Background = Brushes.Transparent };
        };
        var body = new Stack { Spacing = 12 };
        body.Children.Add(flow); body.Children.Add(detail);
        return new Border { Child = body, Padding = new Thickness(12), Style = Themed("ListCard") };
    }

    private static Border Session(ContextSession session)
    {
        var display = new TextBlock { Text = session.Display.Trim(), TextTrimming = TextTrimming.CharacterEllipsis };
        var updated = session.UpdatedAt <= 0 ? Localized.Get("未知")
            : DateTimeOffset.FromUnixTimeSeconds(session.UpdatedAt).ToLocalTime().ToString("MM-dd HH:mm");
        var hit = session.Cache.HitPercent is null ? "—" : $"{session.Cache.HitPercent:0.0}%";
        ToolTipService.SetToolTip(display, Localized.Format("{0}\nrevision {1}\n消息 {2} · 视图 {3} · 命中 {4}\n更新 {5}", session.Session, session.Revision, UsageView.Compact(session.Messages), UsageView.Compact(session.ViewTokens), hit, updated));
        var cells = new UIElement[]
        {
            display,
            new TextBlock { Text = UsageView.Compact(session.Compressions) },
            new TextBlock { Text = UsageView.Compact(session.SavedTokens) }
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
        var grid = new SpacedGrid { ColumnSpacing = 10 };
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
