using System.Globalization;
using System.Text;
using Avalonia;
using Avalonia.Automation;
using Avalonia.Controls;
using Avalonia.Controls.Templates;
using Avalonia.Layout;
using Avalonia.Media;
using TKCode.Arc.DesignTokens;
using Arc = TKCode.Arc;

namespace CodexMixin;

// The context page: "context config get/set" and "context stats" read and write the
// engine snapshot, "context export --compact" loads one session archive on demand.
// The CLI owns validation, storage and the archive database; this page reads and
// writes none of those files directly.
internal sealed partial class MainWindow
{
    private async Task<Control> BuildContextAsync(CancellationToken token)
    {
        var page = new StackPanel { Spacing = 12, MaxWidth = 880, HorizontalAlignment = HorizontalAlignment.Left };
        // Parse failures are contract errors, not "unknown values"; show them verbatim.
        try
        {
            var config = ContextContract.ParseConfig(await cli.QueryAsync(["context", "config", "get", "--json"], token));
            page.Children.Add(ContextSettingsCard(config));
        }
        catch (CliFailure error)
        {
            page.Children.Add(ContextAlert(Localized.Get("上下文设置不可用"), error.Message));
        }
        try
        {
            var stats = ContextContract.ParseStats(await cli.QueryAsync(["context", "stats", "--json"], token));
            page.Children.Add(ContextStatsView(stats, token));
        }
        catch (CliFailure error)
        {
            page.Children.Add(ContextAlert(Localized.Get("上下文统计不可用"), error.Message));
        }
        return page;
    }

    private static Arc.Alert ContextAlert(string title, string message, Control? action = null)
    {
        var body = new StackPanel { Spacing = 8 };
        body.Children.Add(new TextBlock { Text = message, TextWrapping = TextWrapping.Wrap });
        if (action is not null) body.Children.Add(action);
        return new Arc.Alert { Tone = Arc.AlertTone.Danger, Title = title, Content = body };
    }

    private static TextBlock ContextCaption(string text)
    {
        var caption = new TextBlock { Text = text, TextWrapping = TextWrapping.Wrap };
        Arc.ArcTokens.Bind(caption, TextBlock.ForegroundProperty, ArcDesignTokenKind.TextSecondary);
        Arc.ArcTokens.Bind(caption, TextBlock.FontSizeProperty, ArcDesignTokenKind.TypeSM);
        return caption;
    }

    private static Arc.NumberField ContextNumber(string label, decimal value, decimal minimum, decimal maximum, decimal step, int digits = 0) => new()
    {
        Label = label,
        Minimum = minimum,
        Maximum = maximum,
        Step = step,
        MaximumFractionDigits = digits,
        UseGrouping = false,
        HorizontalAlignment = HorizontalAlignment.Stretch,
        Value = value
    };

    private Control ContextSettingsCard(ContextConfig config)
    {
        var stored = config.Settings;
        var enabled = new Arc.Switch { Content = Localized.Get("启用上下文引擎"), IsChecked = stored.Enabled };
        AutomationProperties.SetAutomationId(enabled, "context-enabled");
        var growth = ContextNumber(Localized.Get("增长阈值（tokens）"), stored.GrowthThresholdTokens, 1, 9_000_000_000_000_000, 1000);
        var pressure = ContextNumber(Localized.Get("压力比例"), (decimal)stored.PressureRatio, 0, 1, 0.05m, 4);
        var urgent = ContextNumber(Localized.Get("紧急比例"), (decimal)stored.UrgentRatio, 0, 1, 0.05m, 4);
        var messages = ContextNumber(Localized.Get("保护最近消息数"), stored.ProtectedMessages, 1, 100_000, 1);
        var protectedTokens = ContextNumber(Localized.Get("保护最近 tokens"), stored.ProtectedTokens, 0, 9_000_000_000_000_000, 1000);
        var rounds = ContextNumber(Localized.Get("最大内部轮数"), stored.MaxInternalRounds, 1, 10, 1);
        var window = new Arc.Input
        {
            Label = Localized.Get("上下文窗口（tokens）"),
            Description = Localized.Get("留空表示不指定，由模型目录决定。"),
            Text = stored.ContextWindowTokens?.ToString(CultureInfo.InvariantCulture) ?? "",
            HorizontalAlignment = HorizontalAlignment.Stretch
        };

        var fields = new Grid { ColumnSpacing = 12, RowSpacing = 12 };
        fields.ColumnDefinitions.Add(new ColumnDefinition(1, GridUnitType.Star));
        fields.ColumnDefinitions.Add(new ColumnDefinition(1, GridUnitType.Star));
        Control[] controls = [growth, pressure, urgent, messages, protectedTokens, rounds, window];
        for (var index = 0; index < controls.Length; index++)
        {
            if (index % 2 == 0) fields.RowDefinitions.Add(new RowDefinition(GridLength.Auto));
            Grid.SetColumn(controls[index], index % 2);
            Grid.SetRow(controls[index], index / 2);
            fields.Children.Add(controls[index]);
        }

        // Attached after the initial values so only user edits mark the draft.
        enabled.IsCheckedChanged += (_, _) => draftDirty = true;
        foreach (var field in new[] { growth, pressure, urgent, messages, protectedTokens, rounds })
            field.ValueChanged += (_, _) => draftDirty = true;
        window.TextChanged += (_, _) => draftDirty = true;

        var status = ContextCaption("");
        var save = new Arc.Button { Content = Localized.Get("保存设置"), HorizontalAlignment = HorizontalAlignment.Left };
        AutomationProperties.SetAutomationId(save, "context-save");
        save.Click += async (_, _) =>
        {
            long? windowTokens = null;
            if (!string.IsNullOrWhiteSpace(window.Text))
            {
                if (!long.TryParse(window.Text.Trim(), NumberStyles.None, CultureInfo.InvariantCulture, out var parsed) || parsed <= 0)
                {
                    window.ErrorMessage = Localized.Get("请输入正整数，或留空。");
                    return;
                }
                windowTokens = parsed;
            }
            window.ErrorMessage = null;
            var edited = stored with
            {
                Enabled = enabled.IsChecked == true,
                GrowthThresholdTokens = (long)growth.Value,
                PressureRatio = (double)pressure.Value,
                UrgentRatio = (double)urgent.Value,
                ProtectedMessages = (long)messages.Value,
                ProtectedTokens = (long)protectedTokens.Value,
                MaxInternalRounds = (long)rounds.Value,
                ContextWindowTokens = windowTokens
            };
            if (ContextContract.SaveArguments(stored, edited) is not { } arguments)
            {
                draftDirty = false;
                status.Text = Localized.Get("没有需要保存的更改。");
                return;
            }
            await RunActionAsync(arguments);
        };

        var body = new StackPanel { Spacing = 16 };
        body.Children.Add(enabled);
        body.Children.Add(fields);
        body.Children.Add(save);
        body.Children.Add(status);
        return new Arc.ExpandableCard
        {
            Header = Localized.Get("上下文引擎"),
            Description = Localized.Get("保存更改后自动重启运行中的网关并生效。"),
            IsExpanded = true,
            Content = body
        };
    }

    private Control ContextStatsView(ContextStats stats, CancellationToken token)
    {
        var totals = stats.Totals;
        var cache = totals.Cache;
        var tiles = new Grid { ColumnSpacing = 12, RowSpacing = 12 };
        tiles.ColumnDefinitions.Add(new ColumnDefinition(1, GridUnitType.Star));
        tiles.ColumnDefinitions.Add(new ColumnDefinition(1, GridUnitType.Star));
        tiles.RowDefinitions.Add(new RowDefinition(GridLength.Auto));
        tiles.RowDefinitions.Add(new RowDefinition(GridLength.Auto));
        Control[] metrics =
        [
            new Arc.MetricCard { Label = Localized.Get("会话"), Value = totals.Sessions, Context = Localized.Format("{0:N0} 条归档消息", totals.Messages) },
            new Arc.MetricCard { Label = Localized.Get("压缩次数"), Value = totals.Compressions },
            new Arc.MetricCard { Label = Localized.Get("节省 tokens"), Value = totals.SavedTokens, Context = Localized.Format("当前视图 {0:N0}", totals.ViewTokens) },
            cache.HitPercent is { } hit
                ? new Arc.MetricCard
                {
                    Label = Localized.Get("缓存命中"), Value = hit, Suffix = "%", Decimals = 1,
                    Context = Localized.Format("{0:N0} 轮 · 缓存 {1:N0} / 输入 {2:N0} tokens", cache.Turns, cache.CachedTokens, cache.InputTokens)
                }
                : new Arc.Card { Title = Localized.Get("缓存命中"), Description = Localized.Get("暂无可观测的缓存轮次") }
        ];
        for (var index = 0; index < metrics.Length; index++)
        {
            Grid.SetColumn(metrics[index], index % 2);
            Grid.SetRow(metrics[index], index / 2);
            tiles.Children.Add(metrics[index]);
        }

        var inspector = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        CancellationTokenSource? archiveQuery = null;
        async Task InspectAsync(ContextSession session)
        {
            archiveQuery?.Cancel();
            var request = CancellationTokenSource.CreateLinkedTokenSource(token);
            archiveQuery = request;
            inspector.Content = new Arc.Skeleton { Lines = 4, Label = Localized.Get("正在读取归档…") };
            try
            {
                var document = await cli.QueryAsync(["context", "export", "--compact", $"--session={session.Session}"], request.Token);
                request.Token.ThrowIfCancellationRequested();
                var detail = ContextContract.ParseArchive(document);
                if (detail.Session != session.Session) throw new CliFailure(Localized.Get("CLI 返回了其他会话的归档。"));
                inspector.Content = ContextArchiveView(session, detail);
            }
            catch (OperationCanceledException) { }
            catch (CliFailure error)
            {
                if (ReferenceEquals(archiveQuery, request) && !request.IsCancellationRequested)
                {
                    var retry = new Arc.Button { Content = Localized.Get("重试"), Variant = Arc.ButtonVariant.Secondary, HorizontalAlignment = HorizontalAlignment.Left };
                    retry.Click += async (_, _) => await InspectAsync(session);
                    inspector.Content = ContextAlert(Localized.Get("归档读取失败"), error.Message, retry);
                }
            }
            finally
            {
                if (ReferenceEquals(archiveQuery, request)) archiveQuery = null;
                request.Dispose();
            }
        }

        var sessionBody = new StackPanel { Spacing = 12 };
        if (stats.Sessions.Length == 0)
        {
            sessionBody.Children.Add(new Arc.EmptyState
            {
                Size = Arc.ControlSize.Small,
                Title = Localized.Get("暂无会话"),
                Description = stats.Enabled ? Localized.Get("网关处理请求后，会话会出现在这里。") : Localized.Get("启用上下文引擎后，会话会出现在这里。")
            });
        }
        else
        {
            var table = new Arc.Table
            {
                Caption = Localized.Get("按会话统计"),
                EmptyMessage = Localized.Get("暂无会话"),
                ItemsSource = stats.Sessions,
                Height = Math.Clamp(stats.Sessions.Length * 44 + 60, 140, 420)
            };
            table.Columns.Add(Arc.DataTableColumn.Create<ContextSession>(Localized.Get("会话"), s => s.Display.Trim()));
            table.Columns.Add(Arc.DataTableColumn.Create<ContextSession>(Localized.Get("压缩"), s => s.Compressions));
            table.Columns.Add(Arc.DataTableColumn.Create<ContextSession>(Localized.Get("节省 tokens"), s => s.SavedTokens));
            table.Columns.Add(Arc.DataTableColumn.Create<ContextSession>(Localized.Get("缓存命中"), s => s.Cache.HitPercent));
            table.Columns[3].Formatter = value => value is double percent ? $"{percent:0.0}%" : "—";
            table.Columns.Add(Arc.DataTableColumn.Create<ContextSession>(Localized.Get("更新"), s => s.UpdatedAt));
            table.Columns[4].Formatter = value => value is long seconds && seconds > 0
                ? DateTimeOffset.FromUnixTimeSeconds(seconds).ToLocalTime().ToString("MM-dd HH:mm", CultureInfo.CurrentCulture)
                : Localized.Get("未知");
            table.Columns.Add(new Arc.DataTableColumn(Localized.Get("归档"), _ => "")
            {
                IsSortable = false,
                CellTemplate = new FuncDataTemplate<ContextSession>((_, _) =>
                {
                    var open = new Arc.Button { Content = Localized.Get("查看"), Variant = Arc.ButtonVariant.Ghost, Size = Arc.ControlSize.Small };
                    // Rows are recycled, so the session is read from the cell's current data context.
                    open.Click += (sender, args) =>
                    {
                        if (((Control)sender!).DataContext is ContextSession session) _ = InspectAsync(session);
                    };
                    return open;
                })
            });
            sessionBody.Children.Add(table);
        }

        var body = new StackPanel { Spacing = 12 };
        body.Children.Add(tiles);
        body.Children.Add(new Arc.ExpandableCard
        {
            Header = Localized.Get("按会话统计"),
            Description = Localized.Format("归档库：{0}", stats.DatabasePath),
            IsExpanded = true,
            Content = sessionBody
        });
        body.Children.Add(inspector);
        return body;
    }

    private Control ContextArchiveView(ContextSession session, ContextArchive archive)
    {
        var messages = archive.History.ToDictionary(row => row.Reference);
        var blocks = archive.Blocks.ToDictionary(row => row.Id);
        // Visible size per message position: a message keeps its own bytes, a block
        // contributes its summary once, at the position where the block ends.
        var visibleAt = new Dictionary<long, long>();
        foreach (var segment in archive.Segments)
        {
            long size;
            if (segment.Kind == "block")
            {
                if (!blocks.TryGetValue(segment.Reference, out var block)) throw new CliFailure(Localized.Get("CLI 归档引用了不存在的压缩块。"));
                size = Encoding.UTF8.GetByteCount(block.Summary);
            }
            else if (messages.TryGetValue(segment.Reference, out var message)) size = message.Bytes;
            else throw new CliFailure(Localized.Get("CLI 归档引用了不存在的消息。"));
            var position = segment.Kind == "block" ? segment.EndPosition : segment.Position;
            visibleAt[position] = visibleAt.GetValueOrDefault(position) + size;
        }

        long original = 0, visible = 0;
        var labels = new List<string> { "0" };
        var originalSeries = new List<double> { 0 };
        var visibleSeries = new List<double> { 0 };
        var stride = Math.Max(1, (int)Math.Ceiling(archive.History.Length / 400d));
        for (var index = 0; index < archive.History.Length; index++)
        {
            var message = archive.History[index];
            original += message.Bytes;
            visible += visibleAt.GetValueOrDefault(message.Position);
            if ((index + 1) % stride != 0 && index != archive.History.Length - 1) continue;
            labels.Add((index + 1).ToString(CultureInfo.InvariantCulture));
            originalSeries.Add(original / 1024d);
            visibleSeries.Add(visible / 1024d);
        }
        var labelEvery = Math.Max(1, archive.History.Length / 6);
        var chart = new Arc.LineChart
        {
            Label = Localized.Get("上下文增长"),
            Unit = "KiB",
            CategoryLabel = Localized.Get("消息顺序"),
            Curve = Arc.ChartCurve.Linear,
            PlotHeight = 200,
            ItemsSource = labels,
            XAxisFormatter = category => category is string text && int.TryParse(text, out var number) && number % labelEvery == 0 ? text : null
        };
        chart.Series.Add(new Arc.ChartSeries { Key = "original", Name = Localized.Get("原始归档"), IsDashed = true, Values = originalSeries.ToArray() });
        chart.Series.Add(new Arc.ChartSeries { Key = "view", Name = Localized.Get("当前视图"), Values = visibleSeries.ToArray() });

        var body = new StackPanel { Spacing = 16 };
        body.Children.Add(chart);
        body.Children.Add(ContextCaption(Localized.Get("按消息顺序累计 JSON UTF-8 体积。当前视图表示现有压缩的分布，不代表历史模型 token 用量；不含摘要包装文本。")));
        body.Children.Add(new TextBlock { Text = Localized.Get("压缩概览"), FontWeight = FontWeight.SemiBold });
        var active = archive.Blocks.Where(block => block.SupersededBy is null).ToArray();
        if (active.Length > 0) body.Children.Add(ContextCompressions(active, messages));
        else body.Children.Add(ContextCaption(Localized.Get("此会话尚无压缩块。")));
        var remaining = archive.Segments.Count(segment => segment.Kind == "message");
        if (remaining > 0) body.Children.Add(ContextCaption(Localized.Format("尚未压缩 · {0} 条内容", remaining)));
        var earlier = archive.Blocks.Where(block => block.SupersededBy is not null).ToArray();
        if (earlier.Length > 0)
        {
            body.Children.Add(new Arc.ExpandableCard
            {
                Header = Localized.Format("历史压缩（{0}）", earlier.Length),
                Content = ContextCompressions(earlier, messages)
            });
        }
        return new Arc.ExpandableCard
        {
            Header = session.Display.Trim(),
            Description = Localized.Format("当前视图 {0:N0} token · 已节省 {1:N0} token", session.ViewTokens, session.SavedTokens),
            IsExpanded = true,
            Content = body
        };
    }

    private static Control ContextCompressions(ContextArchiveBlock[] blocks, Dictionary<string, ContextArchiveMessage> messages)
    {
        var list = new Arc.Accordion { ExpandMode = Arc.AccordionExpandMode.Multiple, ExpandedIndex = -1 };
        foreach (var block in blocks)
        {
            var topic = string.IsNullOrEmpty(block.Topic) ? Localized.Get("精简摘要") : block.Topic;
            var sources = new WrapPanel { Orientation = Orientation.Horizontal };
            var covered = block.References.Select(reference => messages.GetValueOrDefault(reference)
                ?? new ContextArchiveMessage(reference, 0, 0, "other", null));
            var groups = covered.GroupBy(message => (message.Kind, message.Tool))
                .OrderByDescending(group => group.Count()).ThenBy(group => group.Key.Kind).ThenBy(group => group.Key.Tool).ToArray();
            foreach (var group in groups.Take(6))
            {
                var (kind, tool) = group.Key;
                var label = tool?.Split('.').Last().Split("__").Last() ?? kind switch
                {
                    "user" => Localized.Get("用户消息"),
                    "assistant" => Localized.Get("助手消息"),
                    "function_call" => Localized.Get("工具调用"),
                    "function_call_output" => Localized.Get("工具结果"),
                    "reasoning" => Localized.Get("推理"),
                    _ => Localized.Get("其他")
                };
                sources.Children.Add(new Arc.Badge { Content = $"{label} ×{group.Count()}", Size = Arc.BadgeSize.Small, Margin = new Thickness(0, 0, 6, 6) });
            }
            if (groups.Length > 6)
                sources.Children.Add(new Arc.Badge { Content = Localized.Format("其他 {0} 条", groups.Skip(6).Sum(group => group.Count())), Size = Arc.BadgeSize.Small });
            var detail = new StackPanel { Spacing = 8 };
            detail.Children.Add(sources);
            detail.Children.Add(new Arc.Textarea
            {
                Label = Localized.Get("摘要"),
                Text = block.Summary,
                IsReadOnly = true,
                TextWrapping = TextWrapping.Wrap,
                FrameMinHeight = 80
            });
            list.Items.Add(new Arc.AccordionItem
            {
                Header = $"{topic} · {Localized.Format("{0} 条内容 → 1 条摘要", block.References.Length)}",
                Content = detail
            });
        }
        return list;
    }
}
