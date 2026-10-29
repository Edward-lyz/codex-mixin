using System.Text.Json;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace CodexMixin;

// Renders `usage --daily` and `usage --turns` JSON for the console usage page:
// totals, a daily stacked bar chart, and the per-reply ledger.
internal static class UsageView
{
    private const double PlotHeight = 120;
    private const long DayMs = 86_400_000;
    // Prompts below the gateway's 8192-token verdict size are too small for
    // provider caches, so their misses are not flagged.
    private const long CacheVerdictMinPrompt = 8_192;
    private const double CacheLowPercent = 50;
    private const int SeriesColors = 4;

    private static long Count(JsonElement row, string key) =>
        row.TryGetProperty(key, out var value) && value.ValueKind == JsonValueKind.Number ? value.GetInt64() : 0;

    private static double? Number(JsonElement row, string key) =>
        row.TryGetProperty(key, out var value) && value.ValueKind == JsonValueKind.Number ? value.GetDouble() : null;

    private static long Tokens(JsonElement row) =>
        Count(row, "input_tokens") + Count(row, "cache_read_tokens") + Count(row, "cache_creation_tokens") + Count(row, "output_tokens");

    internal static string Compact(double value)
    {
        foreach (var (scale, suffix) in new[] { (1e18, "E"), (1e15, "P"), (1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K") })
            if (Math.Abs(value) >= (scale == 1e3 ? scale : scale * 0.99995)) return $"{value / scale:0.0}{suffix}";
        return $"{value:0}";
    }

    internal static UIElement Summary(JsonElement[] daily, JsonElement[] turns, Func<string, string, UIElement?, Border> card)
    {
        var cacheRows = daily.Where(row => Number(row, "cache_hit_percent") != null).ToArray();
        var cached = cacheRows.Sum(row => Count(row, "cache_read_tokens"));
        var prompt = cached + cacheRows.Sum(row => Count(row, "input_tokens") + Count(row, "cache_creation_tokens"));
        var mismatches = turns.Count(turn => turn.TryGetProperty("model_matches", out var match) && match.ValueKind == JsonValueKind.False);
        var grid = new Grid { ColumnSpacing = 12 };
        var metrics = new (string Title, string Value, string Note)[]
        {
            ("请求次数", Compact(daily.Sum(row => Count(row, "request_count"))), $"{daily.Select(row => Json.Text(row, "provider_id")).Distinct().Count()} 个 Provider"),
            ("Tokens", Compact(daily.Sum(Tokens)), $"输出 {Compact(daily.Sum(row => Count(row, "output_tokens")))}"),
            ("缓存命中", prompt > 0 ? $"{cached * 100.0 / prompt:0.0}%" : "—", $"命中 {Compact(cached)} tokens"),
            ("模型不一致", mismatches.ToString(), mismatches > 0 ? "上游返回了其他模型" : "所有回复与请求一致"),
        };
        for (var index = 0; index < metrics.Length; index++)
        {
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var (title, value, note) = metrics[index];
            var body = new StackPanel { Spacing = 2 };
            body.Children.Add(new TextBlock { Text = value, FontSize = 24, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
            body.Children.Add(new TextBlock { Text = note, Style = Themed("Caption") });
            var tile = card(title, "", body);
            Grid.SetColumn(tile, index);
            grid.Children.Add(tile);
        }
        return grid;
    }

    /// Stacked daily bars; `split` stacks one segment per provider.
    internal static UIElement Chart(JsonElement[] daily, IReadOnlyDictionary<string, string> providerNames, int days, bool split, Func<JsonElement, long> metric, string title)
    {
        var today = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() / DayMs;
        var (series, buckets, values, totalValue, max) = UsageBuckets.Build(daily, days, split, today, metric, Tokens);
        var plot = new Grid { Height = PlotHeight + 18, ColumnSpacing = 3 };
        for (var index = 0; index < buckets.Length; index++)
        {
            plot.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var day = buckets[index];
            var column = new StackPanel { VerticalAlignment = VerticalAlignment.Bottom };
            var total = 0L;
            // Bottom-up stacking: the largest series sits at the base.
            for (var seriesIndex = series.Length - 1; seriesIndex >= 0; seriesIndex--)
            {
                var value = values[index][seriesIndex];
                total += value;
                if (value == 0) continue;
                column.Children.Add(new Border { Height = value * PlotHeight / max, Background = SeriesBrush(seriesIndex) });
            }
            var date = DateTimeOffset.FromUnixTimeMilliseconds(day * DayMs);
            column.Children.Add(new TextBlock
            {
                Text = buckets.Length <= 8 || index % (int)Math.Ceiling(buckets.Length / 8.0) == 0 ? $"{date.Month}/{date.Day}" : "",
                Style = Themed("Caption"), FontSize = 11, HorizontalAlignment = HorizontalAlignment.Center, Margin = new Thickness(0, 4, 0, 0)
            });
            ToolTipService.SetToolTip(column, $"{date.Month}/{date.Day} · {Compact(total)}");
            Grid.SetColumn(column, index);
            plot.Children.Add(column);
        }
        var body = new StackPanel { Spacing = 8 };
        body.Children.Add(new TextBlock { Text = $"{title} · 合计 {Compact(totalValue)}", Style = Themed("Caption") });
        body.Children.Add(plot);
        if (split && series.Length > 0)
        {
            var legend = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 14 };
            for (var index = 0; index < series.Length; index++)
            {
                var item = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
                item.Children.Add(new Border { Width = 8, Height = 8, CornerRadius = new CornerRadius(4), Background = SeriesBrush(index), VerticalAlignment = VerticalAlignment.Center });
                item.Children.Add(new TextBlock { Text = providerNames.GetValueOrDefault(series[index], series[index]), Style = Themed("Caption") });
                legend.Children.Add(item);
            }
            body.Children.Add(legend);
        }
        return body;
    }

    internal static long DailyTokens(JsonElement row) => Tokens(row);
    internal static long DailyRequests(JsonElement row) => Count(row, "request_count");

    private static Brush SeriesBrush(int index) => index < SeriesColors
        ? QuotaView.RingBrush(index)
        : (Brush)Microsoft.UI.Xaml.Application.Current.Resources["ControlStrongStrokeColorDisabledBrush"];

    /// One row per reply, newest first; `onSession` filters to one session.
    internal static ListView Turns(JsonElement[] turns, IReadOnlyDictionary<string, string> providerNames, Action<string> onSession)
    {
        var headers = new[] { "时间", "会话", "模型", "未缓存", "命中缓存", "输出", "思考", "首字", "速度" };
        var list = new ListView
        {
            SelectionMode = ListViewSelectionMode.None, Height = Math.Min(360, 44 + turns.Length * 64),
            Header = Row(headers.Select(text => (UIElement)new TextBlock { Text = text, Style = Themed("Caption") }).ToArray(), false),
            ItemTemplate = (DataTemplate)Microsoft.UI.Xaml.Application.Current.Resources["VirtualRowTemplate"],
            ItemContainerStyle = Themed("ModelRowContainer"),
            ItemsSource = Enumerable.Range(0, turns.Length).ToArray()
        };
        AutomationProperties.SetAutomationId(list, "usage-turns");
        list.ContainerContentChanging += (_, args) =>
        {
            if (args.ItemContainer.ContentTemplateRoot is not ContentControl content) return;
            content.Content = args.InRecycleQueue ? null : Turn(turns[(int)args.Item], providerNames, onSession);
            args.Handled = true;
        };
        return list;
    }

    private static Border Turn(JsonElement turn, IReadOnlyDictionary<string, string> providerNames, Action<string> onSession)
    {
        var session = Json.Text(turn, "session");
        var sessionTitle = Json.OptionalText(turn, "session_display") ?? session;
        var sessionButton = new HyperlinkButton
        {
            Content = new TextBlock { Text = sessionTitle, TextTrimming = TextTrimming.CharacterEllipsis },
            Padding = new Thickness(0), FontFamily = new FontFamily("Consolas"),
            HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch
        };
        AutomationProperties.SetName(sessionButton, sessionTitle);
        ToolTipService.SetToolTip(sessionButton, $"{sessionTitle}\n只看这个会话，再次点击显示全部");
        sessionButton.Click += (_, _) => onSession(session);
        var model = new StackPanel();
        model.Children.Add(new TextBlock { Text = Json.Text(turn, "upstream_model_id"), TextTrimming = TextTrimming.CharacterEllipsis });
        var mismatch = turn.TryGetProperty("model_matches", out var match) && match.ValueKind == JsonValueKind.False;
        var providerId = Json.Text(turn, "provider_id");
        var providerName = providerNames.GetValueOrDefault(providerId, providerId);
        model.Children.Add(new TextBlock
        {
            Text = mismatch ? $"{providerName} · 返回 {Json.OptionalText(turn, "returned_model")}" : providerName,
            Style = Themed(mismatch ? "CautionCaption" : "Caption")
        });
        var promptTokens = Count(turn, "input_tokens") + Count(turn, "cache_read_tokens") + Count(turn, "cache_creation_tokens");
        var hit = Number(turn, "cache_hit_percent");
        var lowCache = hit is < CacheLowPercent && promptTokens >= CacheVerdictMinPrompt;
        var reasoning = Number(turn, "reasoning_tokens") is { } reasoningTokens ? Compact(reasoningTokens)
            : Count(turn, "reasoning_chars") is > 0 and var chars ? $"{Compact(chars)} 字" : "—";
        string Optional(string key) => Number(turn, key) is { } value ? Compact(value) : "—";
        var cells = new UIElement[]
        {
            new TextBlock { Text = DateTimeOffset.FromUnixTimeMilliseconds(Count(turn, "at_ms")).ToLocalTime().ToString("MM-dd HH:mm:ss"), Style = Themed("Mono") },
            sessionButton,
            model,
            new TextBlock { Text = Optional("input_tokens") },
            new TextBlock { Text = hit is { } percent ? $"{Optional("cache_read_tokens")} · {percent:0}%" : Optional("cache_read_tokens"), Style = lowCache ? Themed("CautionCaption") : null },
            new TextBlock { Text = Optional("output_tokens") },
            new TextBlock { Text = reasoning },
            new TextBlock { Text = Number(turn, "ttft_ms") is { } ttft ? $"{ttft:0} ms" : "—" },
            new TextBlock { Text = Number(turn, "output_tps") is { } tps ? $"{tps:0.0} t/s" : "—" },
        };
        return Row(cells, true);
    }

    private static readonly double[] ColumnWeights = [1.3, 0.8, 2.2, 0.9, 1.2, 0.8, 0.8, 0.8, 0.9];

    private static Border Row(UIElement[] cells, bool divider)
    {
        var grid = new Grid { ColumnSpacing = 10 };
        for (var index = 0; index < cells.Length; index++)
        {
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(ColumnWeights[index], GridUnitType.Star) });
            if (cells[index] is FrameworkElement element) element.VerticalAlignment = VerticalAlignment.Center;
            Grid.SetColumn((FrameworkElement)cells[index], index);
            grid.Children.Add(cells[index]);
        }
        return new Border { Child = grid, Padding = new Thickness(16, 8, 16, 8), Style = Themed(divider ? "RowDivider" : "RowPlain"), BorderThickness = divider ? new Thickness(0, 1, 0, 0) : new Thickness(0) };
    }

    private static Style Themed(string key) => (Style)Microsoft.UI.Xaml.Application.Current.Resources[key];
}
