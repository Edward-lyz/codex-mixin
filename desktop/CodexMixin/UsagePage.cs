using System.Globalization;
using System.Text.Json;
using Avalonia;
using Avalonia.Automation;
using Avalonia.Controls;
using Avalonia.Layout;
using TKCode.Arc;
using ArcButton = TKCode.Arc.Button;

namespace CodexMixin;

// The usage page: "usage --daily --json" feeds the cards and the trend chart,
// "usage --turns --json" feeds the reply table. Totals, slots and previous-period
// comparison come from UsageStats / UsageBuckets; this file only presents them.
internal sealed partial class MainWindow
{
    private const int UsageTurnLimit = 5000;
    private const int UsageSeriesLimit = 4;
    private static readonly string UsageSucceeded = Localized.Get("成功");
    private static readonly string UsageFailed = Localized.Get("失败");

    private static (double Value, string Suffix) UsageScale(double value)
    {
        foreach (var (scale, suffix) in new[] { (1e18, "E"), (1e15, "P"), (1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K") })
            if (Math.Abs(value) >= (scale == 1e3 ? scale : scale * 0.99995)) return (value / scale, suffix);
        return (value, "");
    }

    private static string UsageCompact(double value)
    {
        var (scaled, suffix) = UsageScale(value);
        return suffix.Length == 0 ? $"{scaled:0}" : $"{scaled:0.0}{suffix}";
    }

    private static string UsageMoney(double value) => value.ToString("$0.0000", CultureInfo.InvariantCulture);

    private static string UsagePercent(double? value) => value is { } percent ? $"{percent:0.0}%" : "—";

    private static string? UsageRelative(double current, double previous)
    {
        if (previous <= 0) return null;
        return Localized.Format("环比 {0}", $"{(current - previous) / previous * 100:+0;-0;0}%");
    }

    private static string? UsagePoints(double? current, double? previous) =>
        current is { } now && previous is { } before ? Localized.Format("环比 {0}", $"{now - before:+0.0;-0.0;0.0} pp") : null;

    private static string UsageSlotLabel(UsageSlot slot) => slot.Hourly ? $"{slot.Start.ToLocalTime():HH}" : $"{slot.Start.UtcDateTime:M/d}";

    private static string UsageSlotTitle(UsageSlot slot) => slot.Hourly
        ? $"{slot.Start.ToLocalTime():M/d HH:00}–{slot.End.ToLocalTime():HH:00}"
        : Localized.Format("{0:M/d}（UTC 日）", slot.Start.UtcDateTime);

    // A value the CLI could not report is shown as a dash, never as a zero.
    private static Control UsageUnknown(string label, string context)
    {
        var body = new StackPanel { Spacing = 4 };
        body.Children.Add(RouteText(label, secondary: true, size: 12));
        body.Children.Add(RouteText("—", size: 28, strong: true));
        body.Children.Add(RouteText(context, secondary: true, size: 12));
        var card = RouteCard(body, 16);
        card.Width = 236;
        return card;
    }

    private static MetricCard UsageKpi(string label, double value, string prefix, string suffix, bool scale, int decimals, string context, string? change, double[] trend)
    {
        if (scale)
        {
            (value, suffix) = UsageScale(value);
            decimals = suffix.Length > 0 ? 1 : 0;
        }
        var card = new MetricCard { Label = label, Value = value, Prefix = prefix, Suffix = suffix, Decimals = decimals, Context = context, Width = 236 };
        if (change != null) card.Change = change;
        if (trend.Length >= 2 && trend.Any(point => point != 0)) card.Sparkline = trend;
        return card;
    }

    /// Five cards over the same population: every reply attempt in the range
    /// from the CLI's daily totals, each compared with the previous period.
    private static Control UsageSummary(UsageStats current, UsageStats previous, UsageSlot[] slots)
    {
        var hasPrevious = previous.Attempts > 0;
        var comparableCost = current.Costed > 0 && previous.Costed > 0 && current.Unpriced == 0 && previous.Unpriced == 0;
        var panel = new WrapPanel { ItemSpacing = 12, LineSpacing = 12 };
        var unpricedNote = current.Unpriced > 0 ? Localized.Format("{0} 次请求未定价，未计入", UsageCompact(current.Unpriced)) : "";
        if (current.Costed > 0)
            panel.Children.Add(UsageKpi(Localized.Get("预估费用（USD）"), current.Cost, "$", "", false, 4,
                unpricedNote.Length > 0 ? unpricedNote : Localized.Get("按模型公开单价估算"),
                comparableCost ? UsageRelative(current.Cost, previous.Cost) : null,
                slots.Where(slot => slot.Total.Costed > 0).Select(slot => slot.Total.Cost).ToArray()));
        else
            panel.Children.Add(UsageUnknown(Localized.Get("预估费用（USD）"), unpricedNote.Length > 0 ? unpricedNote : Localized.Get("暂无可用估算")));
        panel.Children.Add(UsageKpi(Localized.Get("请求次数"), current.Attempts, "", "", true, 0,
            current.Errors > 0 ? Localized.Format("其中失败 {0}", UsageCompact(current.Errors)) : "",
            hasPrevious ? UsageRelative(current.Attempts, previous.Attempts) : null, slots.Select(slot => (double)slot.Total.Attempts).ToArray()));
        panel.Children.Add(UsageKpi("Tokens", current.Tokens, "", "", true, 0,
            $"{Localized.Get("缓存输入")} {UsageCompact(current.Cached)} · {Localized.Get("输出")} {UsageCompact(current.Output)}",
            hasPrevious ? UsageRelative(current.Tokens, previous.Tokens) : null, slots.Select(slot => (double)slot.Total.Tokens).ToArray()));
        if (current.CacheHitPercent is { } hit)
            panel.Children.Add(UsageKpi(Localized.Get("缓存命中率"), hit, "", "%", false, 1,
                Localized.Format("命中 {0} tokens", UsageCompact(current.HitRead)),
                hasPrevious ? UsagePoints(current.CacheHitPercent, previous.CacheHitPercent) : null,
                slots.Where(slot => slot.Total.CacheHitPercent != null).Select(slot => slot.Total.CacheHitPercent!.Value).ToArray()));
        else
            panel.Children.Add(UsageUnknown(Localized.Get("缓存命中率"), Localized.Get("暂无数据")));
        if (current.ErrorPercent is { } errorRate)
            panel.Children.Add(UsageKpi(Localized.Get("错误率"), errorRate, "", "%", false, 1,
                Localized.Format("{0} / {1} 次失败", UsageCompact(current.Errors), UsageCompact(current.Attempts)),
                hasPrevious ? UsagePoints(current.ErrorPercent, previous.ErrorPercent) : null,
                slots.Where(slot => slot.Total.Attempts > 0).Select(slot => slot.Total.ErrorPercent ?? 0).ToArray()));
        else
            panel.Children.Add(UsageUnknown(Localized.Get("错误率"), Localized.Get("暂无数据")));
        return panel;
    }

    private static decimal? UsageNumber(JsonElement row, string key) =>
        row.TryGetProperty(key, out var value) && value.ValueKind == JsonValueKind.Number ? (decimal)value.GetDouble() : null;

    private async Task<Control> BuildUsageAsync(CancellationToken token)
    {
        SegmentedControl Segments(string[] labels, int selected)
        {
            var control = new SegmentedControl();
            for (var index = 0; index < labels.Length; index++) control.Items.Add(new SegmentedOption { Value = index, Label = labels[index] });
            control.SelectedIndex = selected;
            return control;
        }
        StackPanel Captioned(string caption, Control control)
        {
            var field = new StackPanel { Spacing = 4 };
            field.Children.Add(RouteText(caption, secondary: true, size: 12));
            field.Children.Add(control);
            return field;
        }
        var range = Segments([Localized.Get("今天"), Localized.Get("7 天"), Localized.Get("30 天")], 1);
        var metric = Segments(["Tokens", Localized.Get("请求次数"), Localized.Get("预估费用（USD）")], 0);
        var grouping = Segments([Localized.Get("构成"), Localized.Get("按 Provider 拆分")], 0);
        var providerFilter = new Select { Label = Localized.Get("供应商"), MinWidth = 200 };
        AutomationProperties.SetAutomationId(range, "usage-range");
        AutomationProperties.SetAutomationId(metric, "usage-metric");
        AutomationProperties.SetAutomationId(grouping, "usage-grouping");
        AutomationProperties.SetAutomationId(providerFilter, "usage-provider");
        string[] providerFilterIds = [""];
        var filters = new WrapPanel { ItemSpacing = 16, LineSpacing = 8 };
        filters.Children.Add(Captioned(Localized.Get("统计范围"), range));
        filters.Children.Add(Captioned(Localized.Get("图表指标"), metric));
        filters.Children.Add(Captioned(Localized.Get("图表分组"), grouping));
        filters.Children.Add(providerFilter);

        JsonElement[] daily = [], previous = [], turns = [];
        JsonElement[]? slotTurns = null;
        UsageSlot[] slots = [];
        UsageSlot? slot = null;
        Dictionary<string, string> providerNames = [];
        long usageRevision = 0;
        string[] modelIds = [""];
        var updatingFilters = false;
        int Days() => range.SelectedIndex switch { 0 => 1, 2 => 30, _ => 7 };
        bool InProvider(JsonElement row) => providerFilter.SelectedIndex <= 0 ||
            Json.Text(row, "provider_id") == providerFilterIds[providerFilter.SelectedIndex];

        var feedback = RouteAlert();
        var summaryHost = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        var chartHost = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        var status = new Select { Label = Localized.Get("状态"), MinWidth = 140 };
        var model = new Select { Label = Localized.Get("模型"), MinWidth = 220 };
        var substituted = new Checkbox { Content = Localized.Get("仅看模型替换"), IsVisible = false, VerticalAlignment = VerticalAlignment.Bottom };
        var grouped = new Checkbox { Content = Localized.Get("按会话分组"), VerticalAlignment = VerticalAlignment.Bottom };
        var chips = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Bottom };
        var tableNote = RouteText("", secondary: true, size: 12);
        AutomationProperties.SetAutomationId(status, "usage-status");
        AutomationProperties.SetAutomationId(model, "usage-model");
        AutomationProperties.SetAutomationId(substituted, "usage-substituted");
        AutomationProperties.SetAutomationId(grouped, "usage-group-session");
        var failedKeys = new HashSet<string>();
        var substitutedKeys = new HashSet<string>();
        var grid = new DataGrid
        {
            Label = Localized.Get("回复明细"), MaxViewportHeight = 520, Density = DataGridDensity.Compact,
            IsRowCheckEnabled = false, CanDeleteRows = false, ShowTotals = false, ShowSelectionStats = false,
            EmptyMessage = Localized.Get("没有符合条件的回复。"),
            RowStyleSelector = row => row is DataGridRow { Key: string key } && failedKeys.Contains(key) ? DataGridTone.Danger
                : row is DataGridRow { Key: string other } && substitutedKeys.Contains(other) ? DataGridTone.Warning : null
        };
        AutomationProperties.SetAutomationId(grid, "usage-turns");
        void Column(string key, string header, double width, DataGridColumnType type = DataGridColumnType.Text, int decimals = 0) =>
            grid.Columns.Add(new DataGridColumn(key, header) { Width = width, Type = type, Decimals = decimals, IsEditable = false });
        Column("time", Localized.Get("时间"), 170);
        Column("status", Localized.Get("状态"), 80);
        Column("session", Localized.Get("会话"), 170);
        Column("provider", Localized.Get("供应商"), 140);
        Column("model", Localized.Get("模型"), 240);
        Column("input", Localized.Get("未缓存"), 100, DataGridColumnType.Number);
        Column("cached", Localized.Get("命中缓存"), 110, DataGridColumnType.Number);
        Column("hit", Localized.Get("命中率"), 90, DataGridColumnType.Percent, 0);
        Column("output", Localized.Get("输出"), 90, DataGridColumnType.Number);
        Column("reasoning", Localized.Get("思考"), 90, DataGridColumnType.Number);
        Column("ttft", Localized.Get("首字（ms）"), 100, DataGridColumnType.Number);
        Column("tps", Localized.Get("速度（t/s）"), 100, DataGridColumnType.Number, 1);
        Column("cost", Localized.Get("费用（USD）"), 120, DataGridColumnType.Currency, 4);

        DataGridRow TurnRow(JsonElement turn, int index)
        {
            var key = $"{index}";
            var failed = UsageStats.IsFailed(turn);
            if (failed) failedKeys.Add(key);
            var returned = UsageStats.IsSubstituted(turn) ? Json.OptionalText(turn, "returned_model") : null;
            if (returned != null) substitutedKeys.Add(key);
            var providerId = Json.Text(turn, "provider_id");
            var session = Json.Text(turn, "session");
            var modelId = Json.Text(turn, "upstream_model_id");
            return new DataGridRow(key)
            {
                { "time", DateTimeOffset.FromUnixTimeMilliseconds(Json.OptionalInteger(turn, "at_ms") ?? 0).ToLocalTime().ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture) },
                { "status", failed ? Localized.Get("失败") : Localized.Get("成功") },
                { "session", Json.OptionalText(turn, "session_display") ?? session },
                { "provider", providerNames.GetValueOrDefault(providerId, providerId) },
                { "model", returned == null ? modelId : $"{modelId} → {returned}" },
                { "input", UsageNumber(turn, "input_tokens") },
                { "cached", UsageNumber(turn, "cache_read_tokens") },
                { "hit", UsageNumber(turn, "cache_hit_percent") },
                { "output", UsageNumber(turn, "output_tokens") },
                { "reasoning", UsageNumber(turn, "reasoning_tokens") },
                { "ttft", UsageNumber(turn, "ttft_ms") },
                { "tps", UsageNumber(turn, "output_tps") },
                { "cost", failed ? null : UsageNumber(turn, "estimated_cost_usd") }
            };
        }

        ArcButton Chip(string text, Action clear)
        {
            var chip = new ArcButton { Content = $"✕ {text}", Variant = ButtonVariant.Secondary, Size = ControlSize.Small };
            ToolTip.SetTip(chip, Localized.Format("清除筛选：{0}", text));
            chip.Click += (_, _) => clear();
            return chip;
        }
        void RenderTurns()
        {
            var scoped = (slot == null ? turns : slotTurns ?? []).Where(InProvider).ToArray();
            var substitutions = scoped.Count(UsageStats.IsSubstituted);
            substituted.IsVisible = substitutions > 0;
            substituted.Content = Localized.Format("仅看模型替换（{0}）", substitutions);
            if (substitutions == 0) substituted.IsChecked = false;
            var failures = scoped.Count(UsageStats.IsFailed);
            updatingFilters = true;
            var statusIndex = Math.Max(0, status.SelectedIndex);
            status.ItemsSource = new[]
            {
                new SelectOption("all", Localized.Format("全部（{0}）", scoped.Length)),
                new SelectOption("ok", Localized.Format("成功（{0}）", scoped.Length - failures)),
                new SelectOption("failed", Localized.Format("失败（{0}）", failures))
            };
            status.SelectedIndex = statusIndex;
            updatingFilters = false;
            var modelId = model.SelectedIndex > 0 ? modelIds[model.SelectedIndex] : null;
            var shown = scoped.Where(turn => status.SelectedIndex switch
                {
                    1 => !UsageStats.IsFailed(turn),
                    2 => UsageStats.IsFailed(turn),
                    _ => true
                })
                .Where(turn => modelId == null || Json.Text(turn, "upstream_model_id") == modelId)
                .Where(turn => substituted.IsChecked != true || UsageStats.IsSubstituted(turn)).ToArray();
            chips.Children.Clear();
            if (slot != null) chips.Children.Add(Chip(UsageSlotTitle(slot), () => { slot = null; slotTurns = null; RenderCharts(); RenderTurns(); }));
            tableNote.Text = slot != null && slotTurns == null ? Localized.Get("正在读取这段时间的回复…")
                : Localized.Format("显示 {0} 条回复", shown.Length) + (slot == null && turns.Length >= UsageTurnLimit ? Localized.Get("（只列出最近 5000 条；点击图表柱子缩小时间范围）") : "");
            failedKeys.Clear();
            substitutedKeys.Clear();
            grid.GroupBy = grouped.IsChecked == true ? ["session"] : [];
            grid.ItemsSource = shown.Select(TurnRow).ToList();
        }
        async Task SelectSlot(UsageSlot? selected)
        {
            slot = selected; slotTurns = null;
            RenderCharts(); RenderTurns();
            if (selected == null) return;
            try
            {
                var loaded = Json.Rows(await cli.QueryAsync(["usage", "--turns", "--json", "--limit", UsageTurnLimit.ToString(CultureInfo.InvariantCulture),
                    "--since-ms", selected.Start.ToUnixTimeMilliseconds().ToString(CultureInfo.InvariantCulture),
                    "--until-ms", selected.End.ToUnixTimeMilliseconds().ToString(CultureInfo.InvariantCulture)], token));
                if (!ReferenceEquals(slot, selected)) return;
                slotTurns = loaded;
                RenderTurns();
            }
            catch (OperationCanceledException) { }
            catch (CliFailure error) when (ReferenceEquals(slot, selected))
            {
                slot = null; RenderCharts(); RenderTurns();
                RouteShow(feedback, Localized.Get("读取回复失败"), error.Message);
            }
        }

        void RenderCharts()
        {
            var byRequests = metric.SelectedIndex == 1;
            var byCost = metric.SelectedIndex == 2;
            var split = grouping.SelectedIndex == 1;
            double Metric(UsageStats stats) => byCost ? stats.Cost : byRequests ? stats.Attempts : stats.Tokens;
            List<(string Key, string Name, double[] Values)> series;
            if (split)
            {
                var totals = new Dictionary<string, double>();
                foreach (var item in slots)
                    foreach (var (provider, stats) in item.ByProvider) totals[provider] = totals.GetValueOrDefault(provider) + Metric(stats);
                var ranked = totals.Where(pair => pair.Value > 0).OrderByDescending(pair => pair.Value).Select(pair => pair.Key).ToArray();
                var named = ranked.Take(UsageSeriesLimit).ToArray();
                series = named.Select(provider => (provider, providerNames.GetValueOrDefault(provider, provider),
                    slots.Select(item => Metric(item.ByProvider.GetValueOrDefault(provider))).ToArray())).ToList();
                if (ranked.Length > named.Length)
                    series.Add(("other", Localized.Get("其他"), slots.Select(item => ranked.Skip(UsageSeriesLimit).Sum(provider => Metric(item.ByProvider.GetValueOrDefault(provider)))).ToArray()));
            }
            else if (byCost) series = [("cost", Localized.Get("预估费用（USD）"), slots.Select(item => item.Total.Cost).ToArray())];
            else if (byRequests)
                series = [("ok", UsageSucceeded, slots.Select(item => (double)item.Total.Requests).ToArray()),
                    ("failed", UsageFailed, slots.Select(item => (double)item.Total.Errors).ToArray())];
            else
                series = [("cached", Localized.Get("缓存输入"), slots.Select(item => (double)item.Total.Cached).ToArray()),
                    ("uncached", Localized.Get("未缓存输入"), slots.Select(item => (double)item.Total.Uncached).ToArray()),
                    ("output", Localized.Get("输出"), slots.Select(item => (double)item.Total.Output).ToArray())];
            var metricName = byCost ? Localized.Get("预估费用（USD）") : byRequests ? Localized.Get("请求次数") : "Tokens";
            var format = byCost ? "$0.0000" : "N0";
            Func<double, string> tick = byCost ? UsageMoney : UsageCompact;
            var points = slots.Select((item, index) => new ChartPoint(UsageSlotLabel(item), series.Count == 0 ? 0 : series.Sum(entry => entry.Values[index]))
            {
                Key = item.Start.ToUnixTimeMilliseconds().ToString(CultureInfo.InvariantCulture), Label = UsageSlotTitle(item), AxisLabel = UsageSlotLabel(item)
            }).ToArray();
            if (byCost && slots.All(item => item.Total.Costed == 0)) points = [];
            Control chart;
            Func<int?> activeIndex;
            if (series.Count <= 1)
            {
                var bar = new BarChart
                {
                    Label = metricName, CategoryLabel = Localized.Get("时间"), ValueFormat = format, TickFormatter = tick, PlotHeight = 220,
                    ShowAverage = false, EmptyLabel = byCost ? Localized.Get("暂无可用费用估算") : Localized.Get("暂无用量数据"), ItemsSource = points
                };
                chart = bar;
                activeIndex = () => bar.ActiveIndex;
            }
            else
            {
                var line = new LineChart
                {
                    Label = metricName, CategoryLabel = Localized.Get("时间"), ValueFormat = format, TickFormatter = tick, PlotHeight = 220,
                    Curve = ChartCurve.Linear, EmptyLabel = Localized.Get("暂无用量数据")
                };
                foreach (var entry in series) line.Series.Add(new ChartSeries { Key = entry.Key, Name = entry.Name });
                line.ItemsSource = points;
                for (var index = 0; index < series.Count; index++) line.Series[index].Values = series[index].Values;
                chart = line;
                activeIndex = () => line.ActiveIndex;
            }
            chart.PointerReleased += async (_, _) =>
            {
                if (activeIndex() is not { } index || index < 0 || index >= slots.Length) return;
                await SelectSlot(ReferenceEquals(slot, slots[index]) ? null : slots[index]);
            };
            var trend = new StackPanel { Spacing = 8 };
            var heading = new Grid { ColumnDefinitions = new ColumnDefinitions("*,Auto"), ColumnSpacing = 12 };
            var total = series.Sum(entry => entry.Values.Sum());
            heading.Children.Add(RouteText(Localized.Format("{0} · 合计 {1}", metricName, byCost && slots.All(item => item.Total.Costed == 0) ? "—" : tick(total)), secondary: true, size: 12));
            if (slot != null)
            {
                var clear = Chip(UsageSlotTitle(slot), () => { slot = null; slotTurns = null; RenderCharts(); RenderTurns(); });
                Grid.SetColumn(clear, 1);
                heading.Children.Add(clear);
            }
            var trendTitle = RouteText(Days() == 1 ? Localized.Get("用量趋势 · 按小时") : Localized.Get("用量趋势 · 按天（UTC）"), size: 15, strong: true);
            AutomationProperties.SetAutomationId(trendTitle, "usage-trend-title");
            trend.Children.Add(trendTitle);
            trend.Children.Add(heading);
            trend.Children.Add(chart);
            trend.Children.Add(RouteText(Localized.Get("点击图表中的时间点，只看这段时间的回复。"), secondary: true, size: 12));
            var layout = new Grid { ColumnDefinitions = new ColumnDefinitions("*"), ColumnSpacing = 16 };
            layout.Children.Add(trend);
            var shares = series.Select(entry => (entry.Name, Sum: entry.Values.Sum())).Where(entry => entry.Sum > 0)
                .Select(entry => new ChartPoint(entry.Name, entry.Sum)).ToArray();
            if (series.Count > 1 && shares.Length > 0)
            {
                layout.ColumnDefinitions = new ColumnDefinitions("*,320");
                var donut = new DonutChart
                {
                    Label = metricName, TotalLabel = Localized.Get("合计"), ValueFormat = format, ItemsSource = shares, Diameter = 180,
                    EmptyLabel = Localized.Get("暂无用量数据")
                };
                Grid.SetColumn(donut, 1);
                layout.Children.Add(donut);
            }
            chartHost.Content = RouteCard(layout);
        }

        void RebuildScope()
        {
            slot = null; slotTurns = null;
            var current = daily.Where(InProvider).ToArray();
            slots = UsageBuckets.Slots(current, turns.Where(InProvider).ToArray(), Days(), DateTimeOffset.Now);
            summaryHost.Content = UsageSummary(UsageStats.Sum(current.Select(UsageStats.Daily)),
                UsageStats.Sum(previous.Where(InProvider).Select(UsageStats.Daily)), slots);
            RenderCharts(); RenderTurns();
        }
        async Task ReadUsage(CancellationToken cancellation)
        {
            var revision = ++usageRevision;
            var days = Days();
            string Arg(int value) => value.ToString(CultureInfo.InvariantCulture);
            var snapshots = await Task.WhenAll(
                cli.QueryAsync(["usage", "--daily", "--json", "--days", Arg(days * 2)], cancellation),
                cli.QueryAsync(["usage", "--turns", "--json", "--days", Arg(days), "--limit", Arg(UsageTurnLimit)], cancellation),
                cli.QueryAsync(["providers", "list", "--json"], cancellation));
            cancellation.ThrowIfCancellationRequested();
            if (revision != usageRevision) return;
            var now = DateTimeOffset.Now;
            (daily, previous) = UsageBuckets.SplitPeriods(Json.Rows(snapshots[0]), days, now);
            var firstMs = (UsageBuckets.Today(now) - days + 1) * UsageBuckets.DayMs;
            turns = Json.Rows(snapshots[1]).Where(turn => turn.TryGetProperty("at_ms", out var at) && at.GetInt64() >= firstMs).ToArray();
            var listedProviders = Json.Rows(Json.Field(snapshots[2], "providers"));
            var byName = listedProviders.GroupBy(provider => Json.Text(provider, "display_name")).ToDictionary(group => group.Key, group => group.Count());
            providerNames = listedProviders.ToDictionary(provider => Json.Text(provider, "id"), provider =>
            {
                var name = Json.Text(provider, "display_name");
                return byName[name] == 1 ? name : $"{name} ({Json.Text(provider, "id")})";
            });
            foreach (var id in daily.Concat(previous).Concat(turns).Select(row => Json.Text(row, "provider_id")).Distinct())
                providerNames.TryAdd(id, id);
            var keepProvider = providerFilter.SelectedIndex > 0 ? providerFilterIds[providerFilter.SelectedIndex] : "";
            var keepModel = model.SelectedIndex > 0 ? modelIds[model.SelectedIndex] : "";
            updatingFilters = true;
            providerFilterIds = new[] { "" }.Concat(providerNames.Keys.OrderBy(id => providerNames[id], StringComparer.Ordinal)).ToArray();
            providerFilter.ItemsSource = providerFilterIds.Select(id => new SelectOption(id, id.Length == 0 ? Localized.Get("全部供应商") : providerNames[id])).ToArray();
            providerFilter.SelectedIndex = Math.Max(0, Array.IndexOf(providerFilterIds, keepProvider));
            modelIds = new[] { "" }.Concat(turns.Select(turn => Json.Text(turn, "upstream_model_id")).Distinct().Order(StringComparer.Ordinal)).ToArray();
            model.ItemsSource = modelIds.Select(id => new SelectOption(id, id.Length == 0 ? Localized.Get("全部模型") : id)).ToArray();
            model.SelectedIndex = Math.Max(0, Array.IndexOf(modelIds, keepModel));
            updatingFilters = false;
            RebuildScope();
        }

        try { await ReadUsage(token); }
        catch (CliFailure error) when (!token.IsCancellationRequested)
        {
            return new Alert { Tone = AlertTone.Danger, Title = Localized.Get("用量不可用"), Content = error.Message };
        }
        range.SelectionChanged += async (_, _) =>
        {
            range.IsEnabled = false;
            try { await ReadUsage(token); feedback.IsOpen = false; }
            catch (OperationCanceledException) { }
            catch (Exception error) when (!token.IsCancellationRequested && error is CliFailure or InvalidOperationException or FormatException)
            { RouteShow(feedback, Localized.Get("读取用量失败"), error.Message); }
            finally { range.IsEnabled = !token.IsCancellationRequested; }
        };
        providerFilter.SelectionChanged += (_, _) => { if (!updatingFilters) RebuildScope(); };
        metric.SelectionChanged += (_, _) => RenderCharts();
        grouping.SelectionChanged += (_, _) => RenderCharts();
        status.SelectionChanged += (_, _) => { if (!updatingFilters) RenderTurns(); };
        model.SelectionChanged += (_, _) => { if (!updatingFilters) RenderTurns(); };
        substituted.CheckedStateChanged += (_, _) => RenderTurns();
        grouped.CheckedStateChanged += (_, _) => RenderTurns();

        var tableBar = new WrapPanel { ItemSpacing = 12, LineSpacing = 8 };
        foreach (var element in new Control[] { status, model, substituted, grouped, chips }) tableBar.Children.Add(element);
        var table = new StackPanel { Spacing = 12 };
        table.Children.Add(RouteText(Localized.Get("回复明细"), size: 15, strong: true));
        table.Children.Add(tableBar);
        table.Children.Add(tableNote);
        table.Children.Add(grid);
        var page = new StackPanel { Spacing = 12 };
        page.Children.Add(filters);
        page.Children.Add(RouteText(Localized.Get("费用按模型公开单价估算（USD），不代表实际账单；无可用估算时显示 —。"), secondary: true, size: 12));
        page.Children.Add(feedback);
        page.Children.Add(summaryHost);
        page.Children.Add(chartHost);
        page.Children.Add(RouteCard(table));
        return page;
    }
}
