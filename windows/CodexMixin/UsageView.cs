using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Shapes;

namespace CodexMixin;

/// One line of the reply table: a session header while grouping, or a reply.
internal sealed record UsageTableRow(JsonElement? Turn, string Session, string Title, UsageStats Stats, int Count, bool Expanded);

// Renders `usage --daily` and `usage --turns` JSON for the console usage page:
// comparable headline cards, the stacked usage chart, and the reply ledger.
internal static class UsageView
{
    private const double PlotHeight = 120;
    private const double RowHeight = 34;
    // Prompts below the gateway's 8192-token verdict size are too small for
    // provider caches, so their misses are not flagged.
    private const long CacheVerdictMinPrompt = 8_192;
    private const double CacheLowPercent = 50;
    private const int SeriesColors = 4;
    internal static readonly string SucceededSeries = Localized.Get("成功");
    internal static readonly string FailedSeries = Localized.Get("失败");

    private static readonly Brush CachedBrush = new SolidColorBrush(Color.FromArgb(255, 0x2B, 0xA8, 0x9C));
    private static readonly Brush UncachedBrush = new SolidColorBrush(Color.FromArgb(255, 0x23, 0x83, 0xE2));
    private static readonly Brush OutputBrush = new SolidColorBrush(Color.FromArgb(255, 0x90, 0x65, 0xB0));

    private static long Count(JsonElement row, string key) =>
        row.TryGetProperty(key, out var value) && value.ValueKind == JsonValueKind.Number ? value.GetInt64() : 0;

    private static double? Number(JsonElement row, string key) =>
        row.TryGetProperty(key, out var value) && value.ValueKind == JsonValueKind.Number ? value.GetDouble() : null;

    internal static string Compact(double value)
    {
        foreach (var (scale, suffix) in new[] { (1e18, "E"), (1e15, "P"), (1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K") })
            if (Math.Abs(value) >= (scale == 1e3 ? scale : scale * 0.99995)) return $"{value / scale:0.0}{suffix}";
        return $"{value:0}";
    }

    internal static string Money(double value) => value.ToString("$0.0000", System.Globalization.CultureInfo.InvariantCulture);

    private static string Percent(double? value) => value is { } percent ? $"{percent:0.0}%" : "—";

    private enum Tone { Neutral, HigherIsBetter, LowerIsBetter }

    private static (string Text, int Sign)? RelativeDelta(double current, double previous)
    {
        if (previous <= 0) return null;
        var change = (current - (double)previous) / previous * 100;
        return ($"{change:+0;-0;0}%", change > 0.5 ? 1 : change < -0.5 ? -1 : 0);
    }

    private static (string Text, int Sign)? PointDelta(double? current, double? previous)
    {
        if (current is not { } now || previous is not { } before) return null;
        var change = now - before;
        return ($"{change:+0.0;-0.0;0.0} pp", change > 0.05 ? 1 : change < -0.05 ? -1 : 0);
    }

    /// Four cards over the same population: every reply attempt in the range
    /// from the CLI's daily totals, each compared with the previous period.
    internal static UIElement Summary(UsageStats current, UsageStats previous, UsageSlot[] slots)
    {
        var hasPrevious = previous.Attempts > 0;
        var cards = new UIElement[]
        {
            Kpi(Localized.Get("预估费用（USD）"), current.Costed > 0 ? Money(current.Cost) : "—",
                current.Costed > 0 && previous.Costed > 0 && current.Unpriced == 0 && previous.Unpriced == 0 ? RelativeDelta(current.Cost, previous.Cost) : null, Tone.Neutral,
                slots.Select(slot => slot.Total.Cost).ToArray(),
                Detail(Localized.Get("按模型公开单价估算"))),
            Kpi(Localized.Get("请求次数"), Compact(current.Attempts), hasPrevious ? RelativeDelta(current.Attempts, previous.Attempts) : null, Tone.Neutral,
                slots.Select(slot => (double)slot.Total.Attempts).ToArray(), Detail(current.Errors > 0 ? Localized.Format("其中失败 {0}", Compact(current.Errors)) : " ")),
            Kpi("Tokens", Compact(current.Tokens), hasPrevious ? RelativeDelta(current.Tokens, previous.Tokens) : null, Tone.Neutral,
                slots.Select(slot => (double)slot.Total.Tokens).ToArray(), ShareBar(current)),
            Kpi(Localized.Get("缓存命中率"), Percent(current.CacheHitPercent), hasPrevious ? PointDelta(current.CacheHitPercent, previous.CacheHitPercent) : null, Tone.HigherIsBetter,
                slots.Where(slot => slot.Total.CacheHitPercent != null).Select(slot => slot.Total.CacheHitPercent!.Value).ToArray(),
                Detail(current.CacheHitPercent == null ? " " : Localized.Format("命中 {0} tokens", Compact(current.HitRead)))),
            Kpi(Localized.Get("错误率"), Percent(current.ErrorPercent), hasPrevious ? PointDelta(current.ErrorPercent, previous.ErrorPercent) : null, Tone.LowerIsBetter,
                slots.Select(slot => slot.Total.ErrorPercent ?? 0).ToArray(), Detail(Localized.Format("{0} / {1} 次失败", Compact(current.Errors), Compact(current.Attempts))),
                warning: (current.ErrorPercent ?? 0) >= 5),
        };
        var grid = new SpacedGrid { ColumnSpacing = 12, RowSpacing = 12 };
        foreach (var card in cards) grid.Children.Add(card);
        void Layout(double width)
        {
            var columns = Math.Clamp((int)(width / 230), 1, cards.Length);
            if (grid.ColumnDefinitions.Count == columns) return;
            grid.ColumnDefinitions.Clear(); grid.RowDefinitions.Clear();
            for (var column = 0; column < columns; column++) grid.ColumnDefinitions.Add(new ColumnDefinition());
            for (var row = 0; row < (cards.Length + columns - 1) / columns; row++) grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            for (var index = 0; index < cards.Length; index++)
            { Grid.SetColumn(cards[index], index % columns); Grid.SetRow(cards[index], index / columns); }
        }
        Layout(1000);
        grid.SizeChanged += (_, args) => Layout(args.NewSize.Width);
        return grid;
    }

    private static TextBlock Detail(string text) => new() { Text = text, Style = Themed("Caption") };

    private static Border Kpi(string title, string value, (string Text, int Sign)? delta, Tone tone, double[] trend, UIElement detail, bool warning = false)
    {
        var body = new SpacedGrid { RowSpacing = 4 };
        for (var row = 0; row < 3; row++) body.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        body.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        body.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(72) });
        body.Children.Add(new TextBlock { Text = title, Style = Themed("Caption") });
        var number = new Stack { Orientation = Orientation.Horizontal, Spacing = 8 };
        var figure = new TextBlock { Text = value, FontSize = 22, FontWeight = FontWeights.SemiBold };
        if (warning) figure.Style = Themed("QuotaPercentCritical");
        number.Children.Add(figure);
        if (delta is { } change)
        {
            var style = (tone, change.Sign) switch
            {
                (Tone.HigherIsBetter, > 0) or (Tone.LowerIsBetter, < 0) => "SuccessCaption",
                (Tone.HigherIsBetter, < 0) or (Tone.LowerIsBetter, > 0) => "CriticalCaption",
                _ => "Caption"
            };
            var text = new TextBlock { Text = Localized.Format("环比 {0}", change.Text), Style = Themed(style), VerticalAlignment = VerticalAlignment.Center };
            ToolTipService.SetToolTip(text, Localized.Get("与上一个等长时段相比"));
            number.Children.Add(text);
        }
        Grid.SetRow(number, 1);
        body.Children.Add(number);
        Grid.SetRow((FrameworkElement)detail, 2);
        body.Children.Add(detail);
        var spark = Sparkline(trend);
        Grid.SetColumn(spark, 1); Grid.SetRowSpan(spark, 3);
        body.Children.Add(spark);
        return new Border { Style = Themed("Card"), Padding = new Thickness(16, 12, 16, 12), Child = body };
    }

    /// A trend line drawn to the space the card gives it.
    private static FrameworkElement Sparkline(double[] values)
    {
        var host = new Grid { Height = 32, VerticalAlignment = VerticalAlignment.Center };
        if (values.Length < 2 || values.All(value => value == 0)) return host;
        var line = new Polyline { Style = Themed("Sparkline") };
        host.Children.Add(line);
        host.SizeChanged += (_, args) =>
        {
            var max = Math.Max(values.Max(), 1e-9);
            var width = args.NewSize.Width;
            var points = new PointCollection();
            for (var index = 0; index < values.Length; index++)
                points.Add(new Point(index * width / (values.Length - 1), 30 - values[index] / max * 28));
            line.Points = points;
        };
        return host;
    }

    /// Cached, uncached and output shares of the token total.
    private static UIElement ShareBar(UsageStats stats)
    {
        var bar = new Grid { Height = 6, Margin = new Thickness(0, 5, 0, 5) };
        bar.SizeChanged += (_, _) => bar.Clip = new RectangleGeometry(new Rect(bar.RenderSize), 3, 3);
        var parts = new (long Value, Brush Brush, string Name)[] { (stats.Cached, CachedBrush, Localized.Get("缓存输入")), (stats.Uncached, UncachedBrush, Localized.Get("未缓存输入")), (stats.Output, OutputBrush, Localized.Get("输出")) };
        if (stats.Tokens == 0) { bar.Background = (Brush)Application.Current.Resources["ControlStrongFillColorDisabledBrush"]; return bar; }
        foreach (var (value, brush, _) in parts.Where(part => part.Value > 0))
        {
            bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(value, GridUnitType.Star) });
            var segment = new Border { Background = brush };
            Grid.SetColumn(segment, bar.ColumnDefinitions.Count - 1);
            bar.Children.Add(segment);
        }
        ToolTipService.SetToolTip(bar, string.Join("\n", parts.Select(part => $"{part.Name} {Compact(part.Value)} · {part.Value * 100.0 / stats.Tokens:0}%")));
        return bar;
    }

    /// Stacked bars per slot: token composition or succeeded/failed requests,
    /// or one segment per provider. Clicking a bar selects its slot.
    internal static UIElement Chart(UsageSlot[] slots, IReadOnlyDictionary<string, string> providerNames, bool requests, bool split, bool cost,
        UsageSlot? selected, Action<UsageSlot?> onSelect)
    {
        double Metric(UsageStats stats) => cost ? stats.Cost : requests ? stats.Attempts : stats.Tokens;
        string FormatMetric(double value) => cost ? Money(value) : Compact(value);
        string[] series;
        Brush[] brushes;
        Func<UsageSlot, double[]> values;
        if (split)
        {
            var totals = new Dictionary<string, double>();
            foreach (var slot in slots)
                foreach (var (provider, stats) in slot.ByProvider) totals[provider] = totals.GetValueOrDefault(provider) + Metric(stats);
            series = totals.Where(pair => pair.Value > 0).OrderByDescending(pair => pair.Value).Select(pair => pair.Key).ToArray();
            brushes = series.Select((_, index) => SeriesBrush(index)).ToArray();
            values = slot => series.Select(provider => Metric(slot.ByProvider.GetValueOrDefault(provider))).ToArray();
        }
        else if (cost)
        {
            series = [Localized.Get("预估费用（USD）")];
            brushes = [(Brush)Application.Current.Resources["AccentFillColorDefaultBrush"]];
            values = slot => [slot.Total.Cost];
        }
        else if (requests)
        {
            series = [SucceededSeries, FailedSeries];
            brushes = [(Brush)Application.Current.Resources["AccentFillColorDefaultBrush"], (Brush)Application.Current.Resources["SystemFillColorCriticalBrush"]];
            values = slot => [slot.Total.Requests, slot.Total.Errors];
        }
        else
        {
            series = [Localized.Get("缓存输入"), Localized.Get("未缓存输入"), Localized.Get("输出")];
            brushes = [CachedBrush, UncachedBrush, OutputBrush];
            values = slot => [slot.Total.Cached, slot.Total.Uncached, slot.Total.Output];
        }
        var cells = slots.Select(values).ToArray();
        var max = cells.Length == 0 ? 1 : cells.Max(row => row.Sum());
        if (max <= 0) max = 1;
        var plot = new SpacedGrid { Height = PlotHeight + 20, ColumnSpacing = slots.Length > 24 ? 2 : 4 };
        var labelEvery = (int)Math.Ceiling(slots.Length / 8.0);
        for (var index = 0; index < slots.Length; index++)
        {
            var slot = slots[index];
            plot.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var bars = new Stack { VerticalAlignment = VerticalAlignment.Bottom };
            // The first series sits at the base of the stack.
            for (var seriesIndex = series.Length - 1; seriesIndex >= 0; seriesIndex--)
            {
                var value = cells[index][seriesIndex];
                if (value > 0) bars.Children.Add(new Border { Height = Math.Max(1, value * PlotHeight / max), Background = brushes[seriesIndex] });
            }
            if (bars.Children.Count > 0) ((Border)bars.Children[0]).CornerRadius = new CornerRadius(2, 2, 0, 0);
            var column = new Grid { Background = new SolidColorBrush(Colors.Transparent) };
            column.RowDefinitions.Add(new RowDefinition { Height = new GridLength(PlotHeight) });
            column.RowDefinitions.Add(new RowDefinition { Height = new GridLength(20) });
            column.Children.Add(bars);
            var label = new TextBlock
            {
                Text = index % labelEvery == 0 ? SlotLabel(slot) : "", Style = Themed("Caption"), FontSize = 11,
                HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Bottom, TextTrimming = TextTrimming.None
            };
            Grid.SetRow(label, 1);
            column.Children.Add(label);
            if (selected != null && selected.Start != slot.Start) column.Opacity = 0.35;
            var lines = new List<string> { SlotTitle(slot) };
            for (var seriesIndex = 0; seriesIndex < series.Length; seriesIndex++)
                if (cells[index][seriesIndex] > 0) lines.Add($"{SeriesName(series[seriesIndex], providerNames)}  {Compact(cells[index][seriesIndex])}");
            lines.Add(Localized.Format("请求 {0} · 失败 {1} · 缓存命中 {2}", Compact(slot.Total.Attempts), Compact(slot.Total.Errors), Percent(slot.Total.CacheHitPercent)));
            lines.Add(selected?.Start == slot.Start ? Localized.Get("再次点击显示全部回复") : Localized.Get("点击只看这段时间的回复"));
            ToolTipService.SetToolTip(column, string.Join("\n", lines));
            column.MouseLeftButtonUp += (_, _) => onSelect(selected?.Start == slot.Start ? null : slot);
            Grid.SetColumn(column, index);
            plot.Children.Add(column);
        }
        var total = cells.Sum(row => row.Sum());
        var header = new Grid();
        header.Children.Add(new TextBlock { Text = Localized.Format("{0} · 合计 {1}", (cost ? Localized.Get("预估费用（USD）") : requests ? Localized.Get("请求次数") : "Tokens"), cost && slots.All(slot => slot.Total.Costed == 0) ? "—" : FormatMetric(total)), Style = Themed("Caption") });
        var legend = new Stack { Orientation = Orientation.Horizontal, Spacing = 14, HorizontalAlignment = HorizontalAlignment.Right };
        for (var index = 0; index < series.Length; index++)
        {
            var item = new Stack { Orientation = Orientation.Horizontal, Spacing = 6 };
            item.Children.Add(new Border { Width = 8, Height = 8, CornerRadius = new CornerRadius(2), Background = brushes[index], VerticalAlignment = VerticalAlignment.Center });
            item.Children.Add(new TextBlock { Text = SeriesName(series[index], providerNames), Style = Themed("Caption") });
            legend.Children.Add(item);
        }
        header.Children.Add(legend);
        var body = new Stack { Spacing = 8 };
        body.Children.Add(header);
        body.Children.Add(plot);
        return body;
    }

    private static string SeriesName(string series, IReadOnlyDictionary<string, string> providerNames) => providerNames.GetValueOrDefault(series, series);

    private static string SlotLabel(UsageSlot slot) => slot.Hourly ? $"{slot.Start.ToLocalTime():HH}" : $"{slot.Start.UtcDateTime:M/d}";

    internal static string SlotTitle(UsageSlot slot) => slot.Hourly
        ? $"{slot.Start.ToLocalTime():M/d HH:00}–{slot.End.ToLocalTime():HH:00}"
        : Localized.Format("{0:M/d}（UTC 日）", slot.Start.UtcDateTime);

    private static Brush SeriesBrush(int index) => index < SeriesColors
        ? QuotaView.RingBrush(index)
        : (Brush)Application.Current.Resources["ControlStrongStrokeColorDisabledBrush"];

    /// Table lines for the shown replies, grouped by session when asked.
    /// Sessions keep the order of their newest reply.
    internal static UsageTableRow[] Rows(JsonElement[] turns, bool group, ISet<string> expanded)
    {
        if (!group) return turns.Select(turn => new UsageTableRow(turn, Json.Text(turn, "session"), "", default, 1, true)).ToArray();
        var rows = new List<UsageTableRow>();
        foreach (var session in turns.GroupBy(turn => Json.Text(turn, "session")))
        {
            var items = session.ToArray();
            var open = expanded.Contains(session.Key);
            rows.Add(new UsageTableRow(null, session.Key, Json.OptionalText(items[0], "session_display") ?? session.Key,
                UsageStats.Sum(items.Select(UsageStats.Turn)), items.Length, open));
            if (open) rows.AddRange(items.Select(turn => new UsageTableRow(turn, session.Key, "", default, 1, true)));
        }
        return rows.ToArray();
    }

    private static readonly string[] Headers = [Localized.Get("时间"), Localized.Get("状态"), Localized.Get("会话"), Localized.Get("模型"), Localized.Get("未缓存"), Localized.Get("命中缓存"), Localized.Get("输出"), Localized.Get("思考"), Localized.Get("首字"), Localized.Get("速度"), Localized.Get("费用（USD）")];
    private static readonly double[] ColumnWeights = [1.2, 0.7, 1.1, 2.2, 0.8, 1.1, 0.7, 0.7, 0.7, 0.8, 0.9];

    internal static UIElement TableHeader() =>
        Line(Headers.Select(text => (UIElement)new TextBlock { Text = text, Style = Themed("ColumnHeader") }).ToArray(), false);

    /// Virtualized rows; containers are recycled, so only visible rows are built.
    internal static VirtualList Turns(UsageTableRow[] rows, IReadOnlyDictionary<string, string> providerNames, Action<string> onSession, Action<string> onToggle)
    {
        var list = new VirtualList
        {
            SelectionMode = SelectionMode.Single, Focusable = false, Height = Math.Min(14, Math.Max(rows.Length, 1)) * (RowHeight + 1) + 4,
            ItemContainerStyle = Themed("ModelRowContainer"),
            ItemsSource = Enumerable.Range(0, rows.Length).ToArray()
        };
        AutomationProperties.SetAutomationId(list, "usage-turns");
        list.Prepared += (container, item) =>
        {
            var row = rows[(int)item];
            container.Content = row.Turn is { } turn ? Turn(turn, providerNames, onSession) : Group(row, onToggle);
        };
        return list;
    }

    private static UIElement Group(UsageTableRow row, Action<string> onToggle)
    {
        var content = new Stack { Orientation = Orientation.Horizontal, Spacing = 10 };
        content.Children.Add(new FontIcon { Glyph = row.Expanded ? "\uE70D" : "\uE76C", FontSize = 11, VerticalAlignment = VerticalAlignment.Center });
        content.Children.Add(new TextBlock { Text = row.Title, FontWeight = FontWeights.SemiBold, MaxWidth = 360, TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center });
        var summary = Localized.Format("{0} 条 · {1} tokens · 缓存命中 {2}", row.Count, Compact(row.Stats.Tokens), Percent(row.Stats.CacheHitPercent));
        summary += " · " + (row.Stats.Costed > 0 ? Money(row.Stats.Cost) : "—");
        content.Children.Add(new TextBlock { Text = summary, Style = Themed("Caption"), VerticalAlignment = VerticalAlignment.Center });
        if (row.Stats.Errors > 0)
            content.Children.Add(new TextBlock { Text = Localized.Format("{0} 失败", row.Stats.Errors), Style = Themed("CriticalCaption"), VerticalAlignment = VerticalAlignment.Center });
        var button = new Button
        {
            Style = Themed("MenuRow"), Content = content, Height = RowHeight, Padding = new Thickness(16, 0, 16, 0)
        };
        AutomationProperties.SetName(button, $"{row.Title} {summary}");
        button.Click += (_, _) => onToggle(row.Session);
        return new Border { Child = button, Style = Themed("RowDivider") };
    }

    private static Border Turn(JsonElement turn, IReadOnlyDictionary<string, string> providerNames, Action<string> onSession)
    {
        var session = Json.Text(turn, "session");
        var sessionTitle = Json.OptionalText(turn, "session_display") ?? session;
        var sessionButton = new Button
        {
            Style = Themed("LinkButton"),
            Content = new TextBlock { Text = sessionTitle, TextTrimming = TextTrimming.CharacterEllipsis },
            Padding = new Thickness(0), FontFamily = new FontFamily("Cascadia Mono, Consolas"), FontSize = 12,
            HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch
        };
        AutomationProperties.SetName(sessionButton, sessionTitle);
        ToolTipService.SetToolTip(sessionButton, Localized.Format("{0}\n只看这个会话，再次点击显示全部", sessionTitle));
        sessionButton.Click += (_, _) => onSession(session);

        var failed = UsageStats.IsFailed(turn);
        var status = new Stack { Orientation = Orientation.Horizontal, Spacing = 6 };
        status.Children.Add(new Ellipse { Style = Themed(failed ? "DotCritical" : "DotOn"), VerticalAlignment = VerticalAlignment.Center });
        status.Children.Add(new TextBlock { Text = failed ? Localized.Get("失败") : Localized.Get("成功"), Style = Themed(failed ? "CriticalCaption" : "Caption") });
        if (failed) ToolTipService.SetToolTip(status, Json.OptionalText(turn, "error") ?? Localized.Get("上游请求失败"));

        var providerId = Json.Text(turn, "provider_id");
        var providerName = providerNames.GetValueOrDefault(providerId, providerId);
        var modelId = Json.Text(turn, "upstream_model_id");
        var model = new Stack { Orientation = Orientation.Horizontal, Spacing = 6 };
        model.Children.Add(new TextBlock { Text = modelId, TextTrimming = TextTrimming.CharacterEllipsis });
        var tip = $"{providerName} · {modelId}";
        if (UsageStats.IsSubstituted(turn))
        {
            var returned = Json.OptionalText(turn, "returned_model");
            model.Children.Add(new TextBlock { Text = $"→ {returned}", Style = Themed("CautionCaption"), VerticalAlignment = VerticalAlignment.Center });
            tip += Localized.Format("\n上游实际返回 {0}", returned);
        }
        ToolTipService.SetToolTip(model, tip);

        var promptTokens = Count(turn, "input_tokens") + Count(turn, "cache_read_tokens") + Count(turn, "cache_creation_tokens");
        var hit = Number(turn, "cache_hit_percent");
        var lowCache = hit is < CacheLowPercent && promptTokens >= CacheVerdictMinPrompt;
        var reasoning = new TextBlock { Text = Number(turn, "reasoning_tokens") is { } reasoningTokens ? Compact(reasoningTokens) : "—" };
        if (Count(turn, "reasoning_chars") is > 0 and var chars) ToolTipService.SetToolTip(reasoning, Localized.Format("思考内容约 {0} 字", Compact(chars)));
        string Optional(string key) => Number(turn, key) is { } value ? Compact(value) : "—";
        var cells = new UIElement[]
        {
            new TextBlock { Text = DateTimeOffset.FromUnixTimeMilliseconds(Count(turn, "at_ms")).ToLocalTime().ToString("MM-dd HH:mm:ss"), Style = Themed("Mono") },
            status,
            sessionButton,
            model,
            new TextBlock { Text = Optional("input_tokens") },
            new TextBlock { Text = hit is { } percent ? $"{Optional("cache_read_tokens")} · {percent:0}%" : Optional("cache_read_tokens"), Style = lowCache ? Themed("CautionCaption") : null },
            new TextBlock { Text = Optional("output_tokens") },
            reasoning,
            new TextBlock { Text = Number(turn, "ttft_ms") is { } ttft ? $"{ttft:0} ms" : "—" },
            new TextBlock { Text = Number(turn, "output_tps") is { } tps ? $"{tps:0.0} t/s" : "—" },
            new TextBlock { Text = !failed && Number(turn, "estimated_cost_usd") is { } estimate ? Money(estimate) : "—" },
        };
        return Line(cells, true);
    }

    private static Border Line(UIElement[] cells, bool divider)
    {
        var grid = new SpacedGrid { ColumnSpacing = 10 };
        for (var index = 0; index < cells.Length; index++)
        {
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(ColumnWeights[index], GridUnitType.Star) });
            if (cells[index] is FrameworkElement element) element.VerticalAlignment = VerticalAlignment.Center;
            if (cells[index] is TextBlock text) { text.TextTrimming = TextTrimming.CharacterEllipsis; text.FontSize = 12; }
            Grid.SetColumn((FrameworkElement)cells[index], index);
            grid.Children.Add(cells[index]);
        }
        return new Border
        {
            Child = grid, Height = RowHeight, Padding = new Thickness(16, 0, 16, 0),
            Style = Themed(divider ? "RowDivider" : "RowPlain"), BorderThickness = divider ? new Thickness(0, 0, 0, 1) : new Thickness(0)
        };
    }

    private static Style Themed(string key) => (Style)Application.Current.Resources[key];
}
