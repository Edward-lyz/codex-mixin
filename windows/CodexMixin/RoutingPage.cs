using System.Text.Json;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace CodexMixin;

// The 模型路由 page: "route list --json" and "route models --json" read the routes
// and the routable models; "route set" and "route delete" write them. The CLI owns
// validation, storage, and applying the change; this page only edits drafts.
public sealed partial class MainWindow
{
    private const string VirtualRoutePrefix = "auto/";
    private string? routeSelection;

    private static (string Title, string Glyph, string Description) RouteKind(string kind) => kind switch
    {
        "fusion" => ("融合", "\uE8F1", "新建虚拟模型，多个 Panel 模型并行分析，由 Judge 对比，再由 Final 模型回答。"),
        "schedule" => ("定时", "\uE823", "新建虚拟模型，按本地时间路由到不同模型，可避开收费高峰。"),
        _ => ("故障转移", "\uE8AB", "原模型返回 429、5xx、连接失败或超时且尚未输出内容时，按顺序改用备用模型。")
    };

    private static string RouteSummary(JsonElement route) => Json.Text(route, "kind") switch
    {
        "fusion" => $"{Json.Strings(Json.Field(route, "panel_models")).Length} 个 Panel → {Json.Text(route, "final_model")}",
        "schedule" => $"{Json.Rows(Json.Field(route, "time_routes")).Length} 个时段 · 默认 {Json.Text(route, "final_model")}",
        _ => "备用 " + string.Join(" → ", Json.Strings(Json.Field(route, "fallbacks")))
    };

    private async Task RoutingAsync(CancellationToken token)
    {
        var routes = Json.Rows(Json.Field(await Client.QueryAsync(["route", "list", "--json"], token), "routes"));
        var ids = Json.Rows(Json.Field(await Client.QueryAsync(["route", "models", "--json"], token), "models"))
            .Select(row => Json.Text(row, "id"))
            .Where(id => !id.StartsWith(VirtualRoutePrefix, StringComparison.Ordinal)).ToArray();
        ApplyChanges.Content = "保存并重启网关";
        if (ids.Length == 0)
        {
            PageBody.Children.Add(Card("暂无可用模型", "先在「模型与服务」中添加供应商并选择模型，再配置模型路由。"));
            return;
        }
        var models = routes.Select(route => Json.Text(route, "model")).ToArray();
        var editor = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch, VerticalContentAlignment = VerticalAlignment.Top };
        var list = new ListView { SelectionMode = ListViewSelectionMode.Single };
        AutomationProperties.SetAutomationId(list, "route-list");
        foreach (var route in routes)
        {
            var item = new ListViewItem { Tag = Json.Text(route, "model"), Content = RouteRow(route) };
            AutomationProperties.SetName(item, Json.Text(route, "model"));
            list.Items.Add(item);
        }
        ListViewItem? current = null;
        var selecting = false;
        void Select(ListViewItem? item)
        {
            selecting = true;
            list.SelectedItem = item;
            selecting = false;
            current = item;
        }
        void Open(ListViewItem item)
        {
            var model = (string)item.Tag;
            routeSelection = model;
            var route = routes.First(row => Json.Text(row, "model") == model);
            ShowRouteEditor(editor, route, Json.Text(route, "kind"), ids, models);
        }
        list.SelectionChanged += async (_, _) =>
        {
            if (selecting || list.SelectedItem is not ListViewItem item || ReferenceEquals(item, current)) return;
            if (!await ConfirmDiscardAsync("切换路由")) { Select(current); return; }
            current = item;
            Open(item);
        };
        var actions = Row();
        foreach (var kind in new[] { "fusion", "schedule", "failover" })
        {
            var (title, glyph, _) = RouteKind(kind);
            var button = new Button { Content = IconLabel(glyph, $"新建{title}") };
            AutomationProperties.SetAutomationId(button, $"route-new-{kind}");
            AutomationProperties.SetName(button, $"新建{title}");
            button.Click += async (_, _) =>
            {
                try
                {
                    if (!await ConfirmDiscardAsync("新建路由")) return;
                    Select(null);
                    ShowRouteEditor(editor, null, kind, ids, models);
                    MarkDirty();
                }
                catch (Exception error) { ShowNotice("新建路由失败", error.Message, InfoBarSeverity.Error); }
            };
            actions.Children.Add(button);
        }
        var side = new StackPanel { Spacing = 12 };
        side.Children.Add(actions);
        side.Children.Add(routes.Length == 0 ? Text("还没有模型路由。选择一种类型新建。") : list);
        var layout = new Grid { ColumnSpacing = 16 };
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(340) });
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        var sideCard = Card("路由", "请求的模型满足条件时，改由指定模型回答。", side);
        sideCard.VerticalAlignment = VerticalAlignment.Top;
        layout.Children.Add(sideCard);
        Grid.SetColumn(editor, 1);
        layout.Children.Add(editor);
        PageBody.Children.Add(layout);
        var initial = list.Items.Cast<ListViewItem>().FirstOrDefault(item => (string)item.Tag == routeSelection)
            ?? list.Items.Cast<ListViewItem>().FirstOrDefault();
        if (initial == null) editor.Content = Card("新建模型路由", "融合、定时与故障转移都在这里配置，保存后网关自动重启。");
        else { Select(initial); Open(initial); }
    }

    private UIElement RouteRow(JsonElement route)
    {
        var (title, glyph, _) = RouteKind(Json.Text(route, "kind"));
        var row = new Grid { ColumnSpacing = 12, Padding = new Thickness(0, 6, 0, 6) };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.Children.Add(new Border
        {
            Style = Themed("IconTile"), Width = 32, Height = 32,
            Child = new FontIcon { Glyph = glyph, FontSize = 15 }
        });
        var text = new StackPanel { Spacing = 2 };
        text.Children.Add(new TextBlock { Text = Json.Text(route, "model"), TextTrimming = TextTrimming.CharacterEllipsis });
        text.Children.Add(new TextBlock
        {
            Text = $"{title} · {RouteSummary(route)}",
            Style = (Style)Microsoft.UI.Xaml.Application.Current.Resources["SecondaryText"],
            TextTrimming = TextTrimming.CharacterEllipsis
        });
        Grid.SetColumn(text, 1);
        row.Children.Add(text);
        return row;
    }

    // Builds the draft form for one route. Saved fields this page does not edit are
    // carried over from the stored JSON so a save never drops them.
    private void ShowRouteEditor(ContentControl editor, JsonElement? route, string kind, string[] ids, string[] models)
    {
        var saved = route is JsonElement stored ? stored : default;
        var isSaved = route != null;
        var oldModel = isSaved ? Json.Text(saved, "model") : null;
        var (title, _, description) = RouteKind(kind);
        var form = new StackPanel { Spacing = 16 };
        Func<Dictionary<string, object?>> build;
        var tracked = new List<FrameworkElement>();
        if (kind == "failover") build = FailoverForm(form, tracked, saved, isSaved, ids, models, oldModel);
        else build = VirtualRouteForm(form, tracked, saved, isSaved, kind, ids, models);
        foreach (var control in tracked) TrackDraft(control);
        pendingApply = async () =>
        {
            var payload = build();
            payload.Remove("model");
            payload["kind"] = kind;
            var arguments = new List<string> { "route", "set", "--route-json", JsonSerializer.Serialize(payload) };
            if (oldModel != null) arguments.AddRange(["--replace-model", oldModel]);
            routeSelection = kind == "failover" ? (string?)payload["source"] : VirtualRoutePrefix + (string?)payload["id"];
            await ChangeAsync("保存模型路由", arguments.ToArray(), apply: true);
        };
        if (oldModel != null)
        {
            var delete = Button("删除此路由", async () =>
            {
                if (!await ConfirmAsync("删除模型路由？", $"将删除 {oldModel}，保存后网关自动重启。", "删除")) return;
                routeSelection = null;
                await ChangeAsync("删除模型路由", ["route", "delete", "--model", oldModel], apply: true);
            });
            AutomationProperties.SetAutomationId(delete, "route-delete");
            form.Children.Add(delete);
        }
        editor.Content = Card(isSaved ? $"{title} · {oldModel}" : $"新建{title}", description, form);
    }

    private static Dictionary<string, object?> SavedFields(JsonElement saved, bool isSaved) =>
        isSaved ? JsonSerializer.Deserialize<Dictionary<string, object?>>(saved.GetRawText())! : new Dictionary<string, object?>();

    private static string UniqueRouteName(string stem, string[] models)
    {
        for (var index = 1; ; index++)
        {
            var name = index == 1 ? stem : $"{stem}-{index}";
            if (!models.Contains(VirtualRoutePrefix + name)) return name;
        }
    }

    private Func<Dictionary<string, object?>> FailoverForm(StackPanel form, List<FrameworkElement> tracked, JsonElement saved, bool isSaved,
        string[] ids, string[] models, string? oldModel)
    {
        var sources = ids.Where(id => id == oldModel || !models.Contains(id)).ToArray();
        if (sources.Length == 0) throw new InvalidOperationException("所有模型都已配置故障转移。");
        var source = Choice("原模型", sources, isSaved ? Json.Text(saved, "source") : sources[0]);
        AutomationProperties.SetAutomationId(source, "route-source");
        tracked.Add(source);
        form.Children.Add(source);
        var rows = new StackPanel { Spacing = 8 };
        var fallbacks = new List<ComboBox>();
        Button? add = null;
        void Refresh() { if (add != null) add.IsEnabled = fallbacks.Count < 4; }
        void AddFallback(string model)
        {
            var picker = Choice($"备用模型 {fallbacks.Count + 1}", ids, model);
            var row = new StackPanel { Spacing = 8 };
            row.Children.Add(picker);
            row.Children.Add(Button("移除此备用", () =>
            {
                fallbacks.Remove(picker); rows.Children.Remove(row);
                for (var index = 0; index < fallbacks.Count; index++) fallbacks[index].Header = $"备用模型 {index + 1}";
                Refresh(); MarkDirty(); return Task.CompletedTask;
            }));
            fallbacks.Add(picker); rows.Children.Add(row);
            TrackDraft(picker); Refresh();
        }
        if (isSaved) foreach (var model in Json.Strings(Json.Field(saved, "fallbacks"))) AddFallback(model);
        if (fallbacks.Count == 0) AddFallback(ids.FirstOrDefault(id => id != (string)source.SelectedItem) ?? ids[0]);
        add = Button("添加备用模型", () =>
        {
            var used = fallbacks.Select(picker => (string)picker.SelectedItem).Append((string)source.SelectedItem).ToHashSet();
            AddFallback(ids.FirstOrDefault(id => !used.Contains(id)) ?? ids[0]);
            MarkDirty(); return Task.CompletedTask;
        });
        Refresh();
        form.Children.Add(rows);
        form.Children.Add(add);
        form.Children.Add(Text("按顺序尝试备用模型；备用模型失败时不会再触发它自己的故障转移。客户端会看到一条切换提示。"));
        return () =>
        {
            var chosen = fallbacks.Select(picker => (string)picker.SelectedItem).ToArray();
            var origin = (string)source.SelectedItem;
            if (chosen.Length is < 1 or > 4) throw new InvalidOperationException("请配置 1–4 个备用模型。");
            if (chosen.Contains(origin) || chosen.Distinct().Count() != chosen.Length)
                throw new InvalidOperationException("备用模型不能重复，也不能与原模型相同。");
            var payload = SavedFields(saved, isSaved);
            payload["source"] = origin;
            payload["fallbacks"] = chosen;
            return payload;
        };
    }

    private Func<Dictionary<string, object?>> VirtualRouteForm(StackPanel form, List<FrameworkElement> tracked, JsonElement saved, bool isSaved,
        string kind, string[] ids, string[] models)
    {
        var name = Input("名称", isSaved ? Json.Text(saved, "id") : UniqueRouteName(kind, models));
        AutomationProperties.SetAutomationId(name, "route-name");
        var entry = Text("");
        void ShowEntry() => entry.Text = $"Codex 中选择的模型：{VirtualRoutePrefix}{name.Text.Trim()}";
        name.TextChanged += (_, _) => ShowEntry();
        ShowEntry();
        tracked.Add(name);
        form.Children.Add(name);
        form.Children.Add(entry);
        return kind == "fusion" ? FusionFields(form, tracked, saved, isSaved, ids, name) : ScheduleFields(form, tracked, saved, isSaved, ids, name);
    }

    private static string RouteName(TextBox name)
    {
        var value = name.Text.Trim();
        if (value.Length == 0 || value.Contains('/')) throw new InvalidOperationException("名称不能为空，也不能包含 /。");
        return value;
    }

    private Func<Dictionary<string, object?>> FusionFields(StackPanel form, List<FrameworkElement> tracked, JsonElement saved, bool isSaved,
        string[] ids, TextBox name)
    {
        var selected = (isSaved ? Json.Strings(Json.Field(saved, "panel_models")) : ids.Take(3).ToArray()).ToHashSet();
        var panels = new ListView { SelectionMode = ListViewSelectionMode.Multiple, Height = Math.Min(240, 36 + ids.Length * 36), ItemsSource = ids, Header = $"Panel 模型（最多 8 个，共 {ids.Length} 个可选）" };
        AutomationProperties.SetAutomationId(panels, "route-panels");
        var filtering = false;
        void RestoreSelection()
        {
            filtering = true;
            foreach (var item in panels.Items) if (selected.Contains((string)item)) panels.SelectedItems.Add(item);
            filtering = false;
        }
        panels.SelectionChanged += (_, args) =>
        {
            if (filtering) return;
            foreach (string id in args.RemovedItems) selected.Remove(id);
            foreach (string id in args.AddedItems) selected.Add(id);
            MarkDirty();
        };
        RestoreSelection();
        var search = Input("搜索 Panel 模型", "");
        search.TextChanged += (_, _) =>
        {
            filtering = true;
            panels.ItemsSource = ids.Where(id => id.Contains(search.Text, StringComparison.OrdinalIgnoreCase)).ToArray();
            RestoreSelection();
        };
        var defaultPanels = ids.Take(3).ToArray();
        var judge = Choice("Judge 模型", ids, isSaved ? Json.Text(saved, "judge_model") : defaultPanels[0]);
        var final = Choice("Final 模型", ids, isSaved ? Json.Text(saved, "final_model") : defaultPanels.ElementAtOrDefault(1) ?? defaultPanels[0]);
        var minimum = new NumberBox { Header = "最少成功 Panel", Minimum = 1, Maximum = 8, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Inline, Value = isSaved ? Json.Integer(saved, "min_successful") : 1 };
        var timeout = new NumberBox { Header = "单模型超时（秒）", Minimum = 1, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Inline, Value = isSaved ? Json.Integer(saved, "timeout_ms") / 1000.0 : 300 };
        var intermediate = new CheckBox { Content = "在回答中显示 Panel / Judge 中间结果", IsChecked = !isSaved || Json.Flag(saved, "show_intermediate_results") };
        var tools = new CheckBox { Content = "允许 Panel 使用进程内只读工具", IsChecked = !isSaved || Json.Flag(Json.Field(saved, "panel_tools"), "enabled") };
        var advanced = new StackPanel { Spacing = 12 };
        foreach (var element in new FrameworkElement[] { minimum, timeout, intermediate, tools }) advanced.Children.Add(element);
        foreach (var element in new FrameworkElement[] { search, panels, judge, final, new Expander { Header = "高级选项", Content = advanced, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch },
            Text("每个新用户轮次都先由 Panel 并行给出意见、Judge 汇总，再由 Final 模型修改和回复；同一轮内的工具续调直接交给 Final 模型。") })
            form.Children.Add(element);
        tracked.AddRange([judge, final, minimum, timeout, intermediate, tools]);
        return () =>
        {
            var id = RouteName(name);
            var chosen = ids.Where(selected.Contains).ToArray();
            if (chosen.Length is < 1 or > 8) throw new InvalidOperationException("请选择 1–8 个 Panel 模型。");
            if (!double.IsFinite(minimum.Value) || minimum.Value < 1 || minimum.Value > chosen.Length || minimum.Value != Math.Truncate(minimum.Value))
                throw new InvalidOperationException("最少成功 Panel 必须是 1 到 Panel 数量之间的整数。");
            if (!double.IsFinite(timeout.Value) || timeout.Value <= 0) throw new InvalidOperationException("单模型超时必须大于 0。");
            var payload = SavedFields(saved, isSaved);
            payload["id"] = id; payload["mode"] = "orchestration";
            payload["panel_models"] = chosen; payload["judge_model"] = judge.SelectedItem; payload["final_model"] = final.SelectedItem;
            payload["min_successful"] = (int)minimum.Value; payload["timeout_ms"] = (long)Math.Round(timeout.Value * 1000);
            payload["show_intermediate_results"] = intermediate.IsChecked == true;
            var panelTools = isSaved ? JsonSerializer.Deserialize<Dictionary<string, object?>>(Json.Field(saved, "panel_tools").GetRawText())!
                : new Dictionary<string, object?> { ["max_rounds"] = 16, ["max_calls_per_model"] = 64 };
            panelTools["enabled"] = tools.IsChecked == true;
            payload["panel_tools"] = panelTools;
            return payload;
        };
    }

    private Func<Dictionary<string, object?>> ScheduleFields(StackPanel form, List<FrameworkElement> tracked, JsonElement saved, bool isSaved,
        string[] ids, TextBox name)
    {
        var final = Choice("默认模型（未覆盖的时段）", ids, isSaved ? Json.Text(saved, "final_model") : ids[0]);
        AutomationProperties.SetAutomationId(final, "route-default-model");
        tracked.Add(final);
        form.Children.Add(final);
        var rows = new StackPanel { Spacing = 12 };
        var slots = new List<(TimePicker Start, TimePicker End, ComboBox Model)>();
        Button? add = null;
        void Refresh() { if (add != null) add.IsEnabled = slots.Count < 24; }
        void AddSlot(int start, int end, string model)
        {
            var begin = new TimePicker { Time = TimeSpan.FromMinutes(start), ClockIdentifier = "24HourClock", Header = "开始" };
            var finish = new TimePicker { Time = TimeSpan.FromMinutes(end), ClockIdentifier = "24HourClock", Header = "结束" };
            var picker = Choice("时段模型", ids, model);
            var row = new StackPanel { Spacing = 8 };
            var times = Row(); times.Children.Add(begin); times.Children.Add(finish);
            row.Children.Add(times); row.Children.Add(picker);
            var slot = (begin, finish, picker); slots.Add(slot);
            row.Children.Add(Button("移除此时段", () => { slots.Remove(slot); rows.Children.Remove(row); Refresh(); MarkDirty(); return Task.CompletedTask; }));
            TrackDraft(begin); TrackDraft(finish); TrackDraft(picker);
            rows.Children.Add(row);
            Refresh();
        }
        if (isSaved)
            foreach (var slot in Json.Rows(Json.Field(saved, "time_routes")))
                AddSlot((int)Json.Integer(slot, "start_minute"), (int)Json.Integer(slot, "end_minute"), Json.Text(slot, "model"));
        if (slots.Count == 0) AddSlot(540, 1080, ids.ElementAtOrDefault(1) ?? ids[0]);
        add = Button("添加时段", () =>
        {
            var start = slots.Count == 0 ? 540 : (int)slots[^1].End.Time.TotalMinutes;
            AddSlot(start, (start + 240) % 1440, ids[0]);
            MarkDirty(); return Task.CompletedTask;
        });
        Refresh();
        form.Children.Add(rows);
        form.Children.Add(add);
        form.Children.Add(Text("按本机当前时间选择模型。开始时间包含、结束时间不包含；结束早于开始表示跨午夜。时段不能重叠。"));
        return () =>
        {
            var id = RouteName(name);
            if (slots.Count is < 1 or > 24) throw new InvalidOperationException("请配置 1–24 个时段。");
            var ranges = slots.Select(slot => (Start: (int)slot.Start.Time.TotalMinutes, End: (int)slot.End.Time.TotalMinutes, Model: (string)slot.Model.SelectedItem)).ToArray();
            if (ranges.Any(range => range.Start == range.End)) throw new InvalidOperationException("时段的开始和结束时间不能相同。");
            static bool Covers((int Start, int End, string Model) range, int minute) =>
                range.Start < range.End ? minute >= range.Start && minute < range.End : minute >= range.Start || minute < range.End;
            if (Enumerable.Range(0, 1440).Any(minute => ranges.Count(range => Covers(range, minute)) > 1))
                throw new InvalidOperationException("时段不能重叠。");
            var payload = SavedFields(saved, isSaved);
            payload["id"] = id; payload["mode"] = "time_rotation";
            payload["final_model"] = final.SelectedItem;
            payload["time_routes"] = ranges.Select(range => new { start_minute = range.Start, end_minute = range.End, model = range.Model }).ToArray();
            if (!payload.ContainsKey("panel_models")) payload["panel_models"] = Array.Empty<string>();
            if (!payload.ContainsKey("judge_model")) payload["judge_model"] = final.SelectedItem;
            return payload;
        };
    }
}
