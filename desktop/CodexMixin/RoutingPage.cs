using System.Text.Json;
using Avalonia;
using Avalonia.Automation;
using Avalonia.Controls;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Threading;
using TKCode.Arc;
using TKCode.Arc.DesignTokens;
using ArcButton = TKCode.Arc.Button;
using ArcInput = TKCode.Arc.Input;
using ArcTimePicker = TKCode.Arc.TimePicker;

namespace CodexMixin;

// The model routing page: "route list --json" and "route models --json" read the
// routes and routable models; "route set" and "route delete" write them through
// the CLI. The page edits drafts and draws each route on a WorkflowCanvas.
internal sealed partial class MainWindow
{
    private const string RouteVirtualPrefix = "auto/";
    private const int RouteMaxPanels = 8;
    private const int RouteMaxFallbacks = 4;
    private string? routeSelection;
    // Routable models of the current page load, by id: display name and provider id.
    private Dictionary<string, (string Name, string Provider)> routeModels = [];
    private Dictionary<string, JsonElement> routeProviders = [];

    private static (string Title, string Description) RouteKind(string kind) => kind switch
    {
        "fusion" => (Localized.Get("融合"), Localized.Get("每个新用户轮次先由 Panel 模型并行给出意见，Judge 对比汇总，再由 Final 模型修改和回复；同一轮内的工具续调直接交给 Final 模型。")),
        "schedule" => (Localized.Get("定时"), Localized.Get("按本机当前时间选择模型。开始时间包含、结束时间不包含；结束早于开始表示跨午夜，时段不能重叠。")),
        _ => (Localized.Get("故障转移"), Localized.Get("原模型报任何错误，或 30 秒内没有输出时，按顺序改用备用模型；最后一个模型不设超时，报错直接透传。"))
    };

    private string RouteModelName(string id) => routeModels.TryGetValue(id, out var model) ? model.Name : id;
    private string RouteModelProvider(string id) => routeModels.TryGetValue(id, out var model) ? model.Provider : "";
    private string RouteProviderName(string id) =>
        routeProviders.TryGetValue(id, out var provider) ? Json.OptionalText(provider, "display_name") ?? id : id.Length == 0 ? Localized.Get("其他") : id;

    private static string[] RouteStrings(JsonElement row, string key) =>
        row.ValueKind == JsonValueKind.Object && row.TryGetProperty(key, out var value) && value.ValueKind == JsonValueKind.Array ? Json.Strings(value) : [];

    private static long RouteNumber(JsonElement row, string key, long fallback) =>
        row.ValueKind == JsonValueKind.Object ? Json.OptionalInteger(row, key) ?? fallback : fallback;

    private static bool RouteFlag(JsonElement row, string key, bool fallback) =>
        row.ValueKind == JsonValueKind.Object && row.TryGetProperty(key, out var value) && value.ValueKind is JsonValueKind.True or JsonValueKind.False
            ? value.GetBoolean() : fallback;

    private static int RouteLocalMinute() => DateTime.Now.Hour * 60 + DateTime.Now.Minute;

    private static bool RouteCovers(int start, int end, int minute) =>
        start < end ? minute >= start && minute < end : minute >= start || minute < end;

    private static string RouteClock(int minute) => $"{minute % 1440 / 60:00}:{minute % 60:00}";

    private static string RouteActiveScheduleModel(JsonElement route)
    {
        var minute = RouteLocalMinute();
        foreach (var slot in Json.Rows(Json.Field(route, "time_routes")))
            if (RouteCovers((int)Json.Integer(slot, "start_minute"), (int)Json.Integer(slot, "end_minute"), minute)) return Json.Text(slot, "model");
        return Json.Text(route, "final_model");
    }

    private string RouteSummary(JsonElement route) => Json.Text(route, "kind") switch
    {
        "fusion" => Localized.Format("{0} 个 Panel → Judge {1} → Final {2}", RouteStrings(route, "panel_models").Length, RouteModelName(Json.Text(route, "judge_model")), RouteModelName(Json.Text(route, "final_model"))),
        "schedule" => Localized.Format("当前 {0} · {1} 个时段", RouteModelName(RouteActiveScheduleModel(route)), Json.Rows(Json.Field(route, "time_routes")).Length),
        _ => $"{RouteModelName(Json.Text(route, "source"))} → " + string.Join(" → ", RouteStrings(route, "fallbacks").Select(RouteModelName))
    };

    private string RouteTitle(JsonElement route) =>
        Json.Text(route, "kind") == "failover" && string.IsNullOrEmpty(Json.OptionalText(route, "id"))
            ? RouteModelName(Json.Text(route, "source")) : Json.Text(route, "model");

    private static TextBlock RouteText(string text, bool secondary = false, double size = 13, bool strong = false)
    {
        var block = new TextBlock { Text = text, TextWrapping = TextWrapping.Wrap, FontSize = size };
        if (strong) block.FontWeight = FontWeight.SemiBold;
        if (secondary) ArcTokens.Bind(block, TextBlock.ForegroundProperty, ArcDesignTokenKind.TextSecondary);
        return block;
    }

    private static Border RouteCard(Control child, double padding = 16)
    {
        var card = new Border { Child = child, Padding = new Thickness(padding), CornerRadius = new CornerRadius(12) };
        ArcTokens.Bind(card, Border.BackgroundProperty, ArcDesignTokenKind.SurfaceMuted);
        return card;
    }

    private static Alert RouteAlert() => new() { Tone = AlertTone.Danger, IsOpen = false, IsDismissible = true };

    private static void RouteShow(Alert alert, string title, string message)
    {
        alert.Title = title;
        alert.Content = message;
        alert.IsOpen = true;
    }

    private SelectOption[] RouteOptions(string[] ids, IEnumerable<string> chosen) =>
        ids.Concat(chosen.Where(id => !ids.Contains(id))).Distinct().Select(id => new SelectOption(id,
            ids.Contains(id) ? $"{RouteModelName(id)} · {RouteProviderName(RouteModelProvider(id))}" : $"{id} ({Localized.Get("模型不可用")})")
        { Keywords = id }).ToArray();

    private Combobox RouteModelBox(string label, string[] ids, string current, Action<string> changed, string? automationId = null)
    {
        var box = new Combobox
        {
            Label = label, ItemsSource = RouteOptions(ids, [current]),
            Placeholder = Localized.Get("搜索模型"), EmptyText = Localized.Get("没有匹配的模型"), HorizontalAlignment = HorizontalAlignment.Stretch
        };
        if (automationId != null) AutomationProperties.SetAutomationId(box, automationId);
        box.SelectedValue = current;
        box.SelectionChanged += (_, _) =>
        {
            if (box.SelectedValue is not string id || id == current) return;
            current = id;
            draftDirty = true;
            changed(id);
        };
        return box;
    }

    private WorkflowNode RouteNode(string id, string model, string role, object icon, bool active)
    {
        var node = new WorkflowNode(id, RouteModelName(model))
        {
            Description = $"{role} · {RouteProviderName(RouteModelProvider(model))}",
            Icon = icon,
            State = active ? WorkflowNodeState.Done : WorkflowNodeState.Idle
        };
        if (!routeModels.ContainsKey(model))
        {
            node.State = WorkflowNodeState.Failed;
            node.Error = Localized.Get("模型不可用");
        }
        return node;
    }

    private static WorkflowCanvas RouteCanvas(double height) => new()
    {
        IsPanZoomEnabled = false, LayoutDirection = WorkflowLayoutDirection.LeftToRight,
        Height = height, CornerRadius = new CornerRadius(12)
    };

    private async Task<Control> BuildRoutingAsync(CancellationToken token)
    {
        JsonElement[] routes;
        JsonElement[] options;
        JsonElement[] providers;
        try
        {
            var snapshots = await Task.WhenAll(
                cli.QueryAsync(["route", "list", "--json"], token),
                cli.QueryAsync(["route", "models", "--json"], token),
                cli.QueryAsync(["providers", "list", "--json"], token));
            routes = Json.Rows(Json.Field(snapshots[0], "routes"));
            options = Json.Rows(Json.Field(snapshots[1], "models"))
                .Where(row => !Json.Text(row, "id").StartsWith(RouteVirtualPrefix, StringComparison.Ordinal)).ToArray();
            providers = Json.Rows(Json.Field(snapshots[2], "providers"));
        }
        catch (CliFailure error) when (!token.IsCancellationRequested)
        {
            return new Alert { Tone = AlertTone.Danger, Title = Localized.Get("读取路由失败"), Content = error.Message };
        }
        var ids = options.Select(row => Json.Text(row, "id")).Distinct().ToArray();
        routeModels = options.GroupBy(row => Json.Text(row, "id")).ToDictionary(group => group.Key,
            group => (Json.OptionalText(group.First(), "display_name") ?? group.Key, Json.OptionalText(group.First(), "provider_id") ?? ""));
        routeProviders = providers.GroupBy(row => Json.Text(row, "id")).ToDictionary(group => group.Key, group => group.First());
        if (ids.Length == 0)
            return new EmptyState
            {
                Title = Localized.Get("暂无可用模型"),
                Description = Localized.Get("先在「模型与服务」中添加供应商并选择模型，再配置模型路由。")
            };

        var models = routes.Select(route => Json.Text(route, "model")).ToArray();
        var editorHost = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch, VerticalContentAlignment = VerticalAlignment.Top };
        var notice = RouteAlert();
        var rows = new List<(string Model, ArcButton Button)>();
        void Mark(string? model)
        {
            foreach (var (name, button) in rows) button.Variant = name == model ? ButtonVariant.Secondary : ButtonVariant.Ghost;
        }
        async Task<bool> Discard(string title)
        {
            if (!draftDirty) return true;
            return await ConfirmAsync(title, Localized.UseEnglish ? "Discard the unsaved changes?" : "放弃未保存的修改？");
        }
        void Show(JsonElement? route, bool isSaved, string kind)
        {
            editorHost.Content = RouteEditor(route, isSaved, kind, ids, models);
            draftDirty = !isSaved;
            // Controls raise change events while they are laid out; only edits made after that count as a draft.
            Dispatcher.UIThread.Post(() => draftDirty = !isSaved, DispatcherPriority.Background);
            notice.IsOpen = false;
        }
        async Task Draft(string kind, JsonElement? template)
        {
            if (!await Discard(Localized.Get("新建路由"))) return;
            try
            {
                Show(template, false, kind);
                Mark(null);
            }
            catch (InvalidOperationException error) { RouteShow(notice, Localized.Get("新建路由失败"), error.Message); }
        }
        async Task Open(JsonElement route)
        {
            var model = Json.Text(route, "model");
            if (model == routeSelection && !draftDirty) return;
            if (!await Discard(Localized.Get("切换路由"))) return;
            routeSelection = model;
            Mark(model);
            Show(route, true, Json.Text(route, "kind"));
        }

        var side = new StackPanel { Spacing = 8 };
        side.Children.Add(RouteText(Localized.Get("路由"), size: 16, strong: true));
        var create = new WrapPanel { ItemSpacing = 6, LineSpacing = 6 };
        foreach (var kind in new[] { "fusion", "schedule", "failover" })
        {
            var (title, description) = RouteKind(kind);
            var add = new ArcButton { Content = Localized.Format("新建{0}", title), Variant = ButtonVariant.Secondary, Size = ControlSize.Small };
            ToolTip.SetTip(add, description);
            AutomationProperties.SetAutomationId(add, $"route-new-{kind}");
            add.Click += async (_, _) => await Draft(kind, null);
            create.Children.Add(add);
        }
        side.Children.Add(create);
        if (routes.Length == 0) side.Children.Add(RouteText(Localized.Get("还没有模型路由。"), secondary: true));
        foreach (var route in routes)
        {
            var kind = Json.Text(route, "kind");
            var label = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
            label.Children.Add(new Badge
            {
                Content = RouteKind(kind).Title, Size = BadgeSize.Small,
                Tone = RouteFlag(route, "enabled", true) ? BadgeTone.Info : BadgeTone.Neutral
            });
            label.Children.Add(new TextBlock { Text = RouteTitle(route), TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center });
            var row = new ArcButton
            {
                Content = label, Variant = ButtonVariant.Ghost, HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Left
            };
            ToolTip.SetTip(row, $"{RouteKind(kind).Title} · {RouteSummary(route)}");
            AutomationProperties.SetAutomationId(row, "route-item-" + Json.Text(route, "model"));
            row.Click += async (_, _) => await Open(route);
            rows.Add((Json.Text(route, "model"), row));
            side.Children.Add(row);
        }
        var suggestions = RouteSuggestions(ids, routes);
        if (suggestions.Length > 0)
        {
            side.Children.Add(RouteText(Localized.Get("推荐路由"), secondary: true, size: 12));
            foreach (var suggestion in suggestions)
            {
                var kind = Json.Text(suggestion, "kind");
                var name = RouteVirtualPrefix + Json.Text(suggestion, "id");
                var line = new Grid { ColumnDefinitions = new ColumnDefinitions("*,Auto"), ColumnSpacing = 8 };
                var text = RouteText(name, secondary: true);
                text.VerticalAlignment = VerticalAlignment.Center;
                text.TextWrapping = TextWrapping.NoWrap;
                text.TextTrimming = TextTrimming.CharacterEllipsis;
                ToolTip.SetTip(text, $"{RouteKind(kind).Title} · {RouteSummary(suggestion)}");
                var enable = new ArcButton { Content = Localized.Get("启用"), Variant = ButtonVariant.Ghost, Size = ControlSize.Small };
                Grid.SetColumn(enable, 1);
                AutomationProperties.SetAutomationId(enable, "route-suggestion-" + Json.Text(suggestion, "id"));
                enable.Click += async (_, _) => await Draft(kind, suggestion);
                line.Children.Add(text);
                line.Children.Add(enable);
                side.Children.Add(line);
            }
        }

        var layout = new Grid { ColumnDefinitions = new ColumnDefinitions("300,*"), ColumnSpacing = 16 };
        var sideCard = RouteCard(side, 12);
        sideCard.VerticalAlignment = VerticalAlignment.Top;
        var main = new StackPanel { Spacing = 12 };
        main.Children.Add(notice);
        main.Children.Add(editorHost);
        Grid.SetColumn(main, 1);
        layout.Children.Add(sideCard);
        layout.Children.Add(main);

        var initial = routes.FirstOrDefault(route => Json.Text(route, "model") == routeSelection);
        if (initial.ValueKind != JsonValueKind.Object && routes.Length > 0) initial = routes[0];
        if (initial.ValueKind == JsonValueKind.Object)
        {
            routeSelection = Json.Text(initial, "model");
            Mark(routeSelection);
            Show(initial, true, Json.Text(initial, "kind"));
        }
        else
            editorHost.Content = new EmptyState
            {
                Title = Localized.Get("新建模型路由"),
                Description = Localized.Get("融合、定时与故障转移都在这里配置，保存后网关自动重启。")
            };
        return layout;
    }

    /// Built-in route groups: a fusion panel, a never-stuck chain across
    /// providers, and one failover per model name served by several providers.
    /// They stay suggestions until saved, because saving restarts the gateway.
    private JsonElement[] RouteSuggestions(string[] ids, JsonElement[] routes)
    {
        if (ids.Length < 2) return [];
        var drafts = new List<Dictionary<string, object?>>();
        var panels = ids.Take(3).ToArray();
        drafts.Add(new() { ["kind"] = "fusion", ["id"] = "fusion", ["panel_models"] = panels, ["judge_model"] = panels[0], ["final_model"] = panels.ElementAtOrDefault(1) ?? panels[0] });
        var chain = ids.GroupBy(RouteModelProvider).Select(group => group.First()).ToArray();
        if (chain.Length > 1)
            drafts.Add(new() { ["kind"] = "failover", ["id"] = "never-stuck", ["source"] = chain[0], ["fallbacks"] = chain.Skip(1).Take(RouteMaxFallbacks).ToArray() });
        foreach (var group in ids.GroupBy(id => RouteModelName(id).ToLowerInvariant()).OrderBy(group => group.Key, StringComparer.Ordinal))
        {
            var same = group.ToArray();
            if (same.Select(RouteModelProvider).Distinct().Count() < 2) continue;
            drafts.Add(new() { ["kind"] = "failover", ["id"] = group.Key.Replace('/', '-').Replace(' ', '-'), ["source"] = same[0], ["fallbacks"] = same.Skip(1).Take(RouteMaxFallbacks).ToArray() });
        }
        var taken = routes.Select(route => Json.Text(route, "model")).ToHashSet();
        return drafts.Where(draft => !taken.Contains(RouteVirtualPrefix + (string)draft["id"]!))
            .Select(draft => JsonSerializer.SerializeToElement(draft)).ToArray();
    }

    private static Dictionary<string, object?> RouteSavedFields(JsonElement saved, bool hasSource) =>
        hasSource ? JsonSerializer.Deserialize<Dictionary<string, object?>>(saved.GetRawText())! : new Dictionary<string, object?>();

    private static string RouteUniqueName(string stem, string[] models)
    {
        for (var index = 1; ; index++)
        {
            var name = index == 1 ? stem : $"{stem}-{index}";
            if (!models.Contains(RouteVirtualPrefix + name)) return name;
        }
    }

    /// The name field and the model Codex selects for it.
    private ArcInput RouteNameField(StackPanel form, string value, bool optional, Func<string> fallback)
    {
        var name = new ArcInput { Label = Localized.Get("名称"), Text = value };
        AutomationProperties.SetAutomationId(name, "route-name");
        if (optional) name.Placeholder = Localized.Get("可选；留空则直接接管原模型");
        void ShowEntry()
        {
            var text = (name.Text ?? "").Trim();
            name.Description = Localized.Format("Codex 中选择的模型：{0}", text.Length == 0 && optional ? fallback() : RouteVirtualPrefix + text);
        }
        name.TextChanged += (_, _) => { ShowEntry(); draftDirty = true; };
        ShowEntry();
        form.Children.Add(name);
        return name;
    }

    private static string RouteNameValue(ArcInput name)
    {
        var value = (name.Text ?? "").Trim();
        if (value.Length == 0 || value.Contains('/')) throw new InvalidOperationException(Localized.Get("名称不能为空，也不能包含 /。"));
        return value;
    }

    // Builds the draft editor for one route. `route` is the saved route, or a
    // suggestion template when `isSaved` is false. Saved fields this page does
    // not edit are carried over so a save never drops them.
    private Control RouteEditor(JsonElement? route, bool isSaved, string kind, string[] ids, string[] models)
    {
        var saved = route ?? default;
        var oldModel = isSaved ? Json.Text(saved, "model") : null;
        var (title, description) = RouteKind(kind);
        var alert = RouteAlert();
        var canvas = RouteCanvas(220);
        var form = new StackPanel { Spacing = 12 };
        var build = kind switch
        {
            "fusion" => RouteFusion(canvas, form, saved, route != null, isSaved, ids, models),
            "schedule" => RouteSchedule(canvas, form, saved, route != null, isSaved, ids, models),
            _ => RouteFailover(canvas, form, saved, route != null, isSaved, ids, models, oldModel)
        };

        var heading = new StackPanel { Spacing = 4 };
        var titleLine = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        titleLine.Children.Add(RouteText(isSaved ? RouteTitle(saved) : Localized.Format("新建{0}", title), size: 18, strong: true));
        titleLine.Children.Add(new Badge { Content = title, Tone = BadgeTone.Info, Size = BadgeSize.Small, VerticalAlignment = VerticalAlignment.Center });
        heading.Children.Add(titleLine);
        heading.Children.Add(RouteText(description, secondary: true, size: 12));

        var actions = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Top };
        if (oldModel != null)
        {
            var delete = new ArcButton { Content = Localized.Get("删除路由…"), Variant = ButtonVariant.Danger };
            delete.Click += async (_, _) =>
            {
                if (!await ConfirmAsync(Localized.Get("删除模型路由？"), Localized.Format("将删除 {0}，保存后网关自动重启。此操作不可撤销。", oldModel))) return;
                if (await RunActionAsync(["route", "delete", "--model", oldModel], apply: true)) routeSelection = null;
            };
            AutomationProperties.SetAutomationId(delete, "route-delete");
            actions.Children.Add(delete);
        }
        var save = new ArcButton { Content = Localized.Get("保存并重启网关"), Variant = ButtonVariant.Primary };
        save.Click += async (_, _) =>
        {
            Dictionary<string, object?> payload;
            try { payload = build(); }
            catch (InvalidOperationException error) { RouteShow(alert, Localized.Get("保存模型路由失败"), error.Message); return; }
            alert.IsOpen = false;
            payload.Remove("model");
            payload.Remove("activity");
            payload["kind"] = kind;
            var arguments = new List<string> { "route", "set", "--route-json", JsonSerializer.Serialize(payload) };
            if (oldModel != null) arguments.AddRange(["--replace-model", oldModel]);
            var previous = routeSelection;
            routeSelection = payload.TryGetValue("id", out var id) && id is string name && name.Length > 0 ? RouteVirtualPrefix + name : (string?)payload["source"];
            if (!await RunActionAsync(arguments.ToArray(), apply: true)) routeSelection = previous;
        };
        AutomationProperties.SetAutomationId(save, "route-save");
        actions.Children.Add(save);

        var header = new Grid { ColumnDefinitions = new ColumnDefinitions("*,Auto"), ColumnSpacing = 12 };
        Grid.SetColumn(actions, 1);
        header.Children.Add(heading);
        header.Children.Add(actions);
        var body = new StackPanel { Spacing = 16 };
        body.Children.Add(header);
        body.Children.Add(alert);
        body.Children.Add(canvas);
        body.Children.Add(form);
        return RouteCard(body, 20);
    }

    // ---- Fusion: Panel (parallel) -> Judge -> Final ----------------------------

    private Func<Dictionary<string, object?>> RouteFusion(WorkflowCanvas canvas, StackPanel form, JsonElement saved, bool hasSource, bool isSaved,
        string[] ids, string[] models)
    {
        var defaults = ids.Take(3).ToArray();
        var panels = (hasSource ? RouteStrings(saved, "panel_models") : defaults).ToList();
        var judge = hasSource ? Json.Text(saved, "judge_model") : defaults[0];
        var final = hasSource ? Json.Text(saved, "final_model") : defaults.ElementAtOrDefault(1) ?? defaults[0];
        void Draw()
        {
            var graph = new WorkflowGraph();
            var judgeNode = RouteNode("judge", judge, "Judge", ArcIcons.GitBranch, true);
            var finalNode = RouteNode("final", final, "Final", ArcIcons.MessageSquare, true);
            var panelNodes = panels.Select((panel, index) => RouteNode($"panel-{index}", panel, "Panel", ArcIcons.Cloud, true)).ToArray();
            foreach (var node in panelNodes) graph.Nodes.Add(node);
            graph.Nodes.Add(judgeNode);
            graph.Nodes.Add(finalNode);
            foreach (var node in panelNodes) graph.Connect(node, judgeNode);
            graph.Connect(judgeNode, finalNode);
            canvas.Height = Math.Max(180, Math.Max(panels.Count, 1) * 76 + 48);
            canvas.Graph = graph;
        }

        var optionIds = ids.Concat(panels.Where(id => !ids.Contains(id))).Distinct().ToArray();
        var options = RouteOptions(ids, panels);
        var panelBox = new MultiSelect
        {
            Label = Localized.Get("Panel 模型"), ItemsSource = options, IsSearchable = true, MaxVisibleChips = 3,
            SearchPlaceholder = Localized.Get("搜索模型"), EmptyText = Localized.Get("没有匹配的模型"), HorizontalAlignment = HorizontalAlignment.Stretch
        };
        AutomationProperties.SetAutomationId(panelBox, "route-panels");
        for (var index = 0; index < options.Length; index++)
            if (panels.Contains(optionIds[index])) panelBox.SelectedItems!.Add(options[index]);
        void Count() => panelBox.Description = Localized.Format("已选 {0} / {1}", panels.Count, RouteMaxPanels);
        Count();
        panelBox.SelectionChanged += (_, _) =>
        {
            panels = panelBox.SelectedItems!.Cast<object>().Select(item => optionIds[Array.IndexOf(options, item)]).ToList();
            draftDirty = true;
            Count();
            Draw();
        };
        var judgeBox = RouteModelBox(Localized.Get("Judge 模型"), ids, judge, picked => { judge = picked; Draw(); }, "route-judge");
        var finalBox = RouteModelBox(Localized.Get("Final 模型"), ids, final, picked => { final = picked; Draw(); }, "route-final");
        Draw();
        var picks = new Grid { ColumnDefinitions = new ColumnDefinitions("*,*,*"), ColumnSpacing = 12 };
        Grid.SetColumn(judgeBox, 1);
        Grid.SetColumn(finalBox, 2);
        picks.Children.Add(panelBox);
        picks.Children.Add(judgeBox);
        picks.Children.Add(finalBox);
        form.Children.Add(picks);

        var name = RouteNameField(form, hasSource ? Json.OptionalText(saved, "id") ?? "fusion" : RouteUniqueName("fusion", models), false, () => "");
        if (hasSource && !isSaved && models.Contains(RouteVirtualPrefix + name.Text)) name.Text = RouteUniqueName(name.Text!, models);
        var panelTools = hasSource && saved.TryGetProperty("panel_tools", out var tools) && tools.ValueKind == JsonValueKind.Object ? tools : default;
        var minimum = new NumberField { Label = Localized.Get("最少成功 Panel"), Minimum = 1, Maximum = RouteMaxPanels, Value = RouteNumber(saved, "min_successful", 1) };
        var timeout = new NumberField { Label = Localized.Get("单模型超时（秒）"), Minimum = 1, Value = RouteNumber(saved, "timeout_ms", 300_000) / 1000m };
        var intermediate = new Checkbox { Content = Localized.Get("在回答中显示 Panel / Judge 中间结果"), IsChecked = RouteFlag(saved, "show_intermediate_results", true) };
        var readOnlyTools = new Checkbox { Content = Localized.Get("允许 Panel 使用进程内只读工具"), IsChecked = RouteFlag(panelTools, "enabled", true) };
        minimum.ValueChanged += (_, _) => draftDirty = true;
        timeout.ValueChanged += (_, _) => draftDirty = true;
        intermediate.CheckedStateChanged += (_, _) => draftDirty = true;
        readOnlyTools.CheckedStateChanged += (_, _) => draftDirty = true;
        var numbers = new Grid { ColumnDefinitions = new ColumnDefinitions("*,*"), ColumnSpacing = 12 };
        Grid.SetColumn(timeout, 1);
        numbers.Children.Add(minimum);
        numbers.Children.Add(timeout);
        form.Children.Add(RouteText(Localized.Get("高级选项"), secondary: true, size: 12));
        form.Children.Add(numbers);
        form.Children.Add(intermediate);
        form.Children.Add(readOnlyTools);
        return () =>
        {
            var id = RouteNameValue(name);
            if (panels.Count is < 1 or > RouteMaxPanels) throw new InvalidOperationException(Localized.Get("请选择 1–8 个 Panel 模型。"));
            if (minimum.Value < 1 || minimum.Value > panels.Count || minimum.Value != decimal.Truncate(minimum.Value))
                throw new InvalidOperationException(Localized.Get("最少成功 Panel 必须是 1 到 Panel 数量之间的整数。"));
            if (timeout.Value <= 0) throw new InvalidOperationException(Localized.Get("单模型超时必须大于 0。"));
            var payload = RouteSavedFields(saved, hasSource);
            payload["id"] = id; payload["mode"] = "orchestration";
            payload["panel_models"] = panels.ToArray(); payload["judge_model"] = judge; payload["final_model"] = final;
            payload["min_successful"] = (int)minimum.Value; payload["timeout_ms"] = (long)Math.Round(timeout.Value * 1000);
            payload["show_intermediate_results"] = intermediate.IsChecked == true;
            var toolFields = panelTools.ValueKind == JsonValueKind.Object ? JsonSerializer.Deserialize<Dictionary<string, object?>>(panelTools.GetRawText())!
                : new Dictionary<string, object?> { ["max_rounds"] = 16, ["max_calls_per_model"] = 64 };
            toolFields["enabled"] = readOnlyTools.IsChecked == true;
            payload["panel_tools"] = toolFields;
            return payload;
        };
    }

    // ---- Schedule: time slots -> model, then the default ------------------------

    private static TimeOnly RouteTime(int minute) => TimeOnly.FromTimeSpan(TimeSpan.FromMinutes(minute % 1440));

    private Func<Dictionary<string, object?>> RouteSchedule(WorkflowCanvas canvas, StackPanel form, JsonElement saved, bool hasSource, bool isSaved,
        string[] ids, string[] models)
    {
        var fallback = hasSource ? Json.Text(saved, "final_model") : ids[0];
        var slots = hasSource
            ? Json.Rows(Json.Field(saved, "time_routes")).Select(slot => (Start: (int)Json.Integer(slot, "start_minute"), End: (int)Json.Integer(slot, "end_minute"), Model: Json.Text(slot, "model"))).ToList()
            : [(540, 1080, ids.ElementAtOrDefault(1) ?? ids[0])];
        var rows = new StackPanel { Spacing = 8 };
        void Draw()
        {
            var minute = RouteLocalMinute();
            var graph = new WorkflowGraph();
            var root = new WorkflowNode("client", "Codex")
            {
                Icon = ArcIcons.Inbox, HasDefaultInput = false, State = WorkflowNodeState.Done,
                Description = (Localized.UseEnglish ? "Local time " : "本机时间 ") + RouteClock(minute)
            };
            graph.Nodes.Add(root);
            var anyActive = false;
            for (var index = 0; index < slots.Count; index++)
            {
                var slot = slots[index];
                var active = slot.Start != slot.End && RouteCovers(slot.Start, slot.End, minute);
                anyActive |= active;
                var node = RouteNode($"slot-{index}", slot.Model, $"{RouteClock(slot.Start)}–{RouteClock(slot.End)}", ArcIcons.Cloud, active);
                graph.Nodes.Add(node);
                graph.Connect(root, node);
            }
            var other = RouteNode("default", fallback, Localized.Get("其余时间"), ArcIcons.Cloud, !anyActive);
            graph.Nodes.Add(other);
            graph.Connect(root, other);
            canvas.Height = Math.Max(200, (slots.Count + 1) * 76 + 48);
            canvas.Graph = graph;
        }
        void Render()
        {
            rows.Children.Clear();
            for (var index = 0; index < slots.Count; index++)
            {
                var position = index;
                var slot = slots[index];
                var begin = new ArcTimePicker { Label = Localized.Format("时段 {0} 开始", index + 1), IsLabelVisible = false, SelectedTime = RouteTime(slot.Start), MinuteStep = 5, Width = 140 };
                var finish = new ArcTimePicker { Label = Localized.Format("时段 {0} 结束", index + 1), IsLabelVisible = false, SelectedTime = RouteTime(slot.End), MinuteStep = 5, Width = 140 };
                begin.SelectedTimeChanged += (_, _) =>
                {
                    if (begin.SelectedTime is not { } time) return;
                    slots[position] = slots[position] with { Start = time.Hour * 60 + time.Minute };
                    draftDirty = true; Draw();
                };
                finish.SelectedTimeChanged += (_, _) =>
                {
                    if (finish.SelectedTime is not { } time) return;
                    slots[position] = slots[position] with { End = time.Hour * 60 + time.Minute };
                    draftDirty = true; Draw();
                };
                var box = RouteModelBox(Localized.Get("时段模型"), ids, slot.Model, picked => { slots[position] = slots[position] with { Model = picked }; Draw(); }, $"route-slot-model-{position}");
                var remove = new ArcButton { Content = "✕", Variant = ButtonVariant.Ghost, Size = ControlSize.Small, VerticalAlignment = VerticalAlignment.Center };
                ToolTip.SetTip(remove, Localized.Get("移除此时段"));
                AutomationProperties.SetName(remove, Localized.Get("移除此时段"));
                AutomationProperties.SetAutomationId(remove, $"route-slot-remove-{position}");
                remove.Click += (_, _) => { slots.RemoveAt(position); draftDirty = true; Render(); Draw(); };
                var dash = RouteText("–");
                dash.VerticalAlignment = VerticalAlignment.Center;
                var line = new Grid { ColumnDefinitions = new ColumnDefinitions("Auto,Auto,Auto,*,Auto"), ColumnSpacing = 10 };
                Grid.SetColumn(dash, 1); Grid.SetColumn(finish, 2); Grid.SetColumn(box, 3); Grid.SetColumn(remove, 4);
                foreach (var cell in new Control[] { begin, dash, finish, box, remove }) line.Children.Add(cell);
                rows.Children.Add(line);
            }
        }
        form.Children.Add(RouteText(Localized.Get("时段（本机时间）"), secondary: true, size: 12));
        form.Children.Add(rows);
        var add = new ArcButton { Content = Localized.Get("添加时段"), Variant = ButtonVariant.Secondary, Size = ControlSize.Small, HorizontalAlignment = HorizontalAlignment.Left };
        add.Click += (_, _) =>
        {
            if (slots.Count >= 24) return;
            var start = slots.Count == 0 ? 540 : slots[^1].End;
            slots.Add((start, (start + 240) % 1440, ids[0]));
            draftDirty = true; Render(); Draw();
        };
        AutomationProperties.SetAutomationId(add, "route-slot-add");
        form.Children.Add(add);
        form.Children.Add(RouteModelBox(Localized.Get("其余时间"), ids, fallback, picked => { fallback = picked; Draw(); }, "route-default-model"));
        Render();
        Draw();
        var name = RouteNameField(form, hasSource ? Json.OptionalText(saved, "id") ?? "schedule" : RouteUniqueName("schedule", models), false, () => "");
        if (hasSource && !isSaved && models.Contains(RouteVirtualPrefix + name.Text)) name.Text = RouteUniqueName(name.Text!, models);
        return () =>
        {
            var id = RouteNameValue(name);
            if (slots.Count is < 1 or > 24) throw new InvalidOperationException(Localized.Get("请配置 1–24 个时段。"));
            if (slots.Any(range => range.Start == range.End)) throw new InvalidOperationException(Localized.Get("时段的开始和结束时间不能相同。"));
            if (Enumerable.Range(0, 1440).Any(minute => slots.Count(range => RouteCovers(range.Start, range.End, minute)) > 1))
                throw new InvalidOperationException(Localized.Get("时段不能重叠。"));
            var payload = RouteSavedFields(saved, hasSource);
            payload["id"] = id; payload["mode"] = "time_rotation";
            payload["final_model"] = fallback;
            payload["time_routes"] = slots.Select(range => new { start_minute = range.Start, end_minute = range.End, model = range.Model }).ToArray();
            if (!payload.ContainsKey("panel_models")) payload["panel_models"] = Array.Empty<string>();
            if (!payload.ContainsKey("judge_model")) payload["judge_model"] = fallback;
            return payload;
        };
    }

    // ---- Failover: source, then fallbacks in order ------------------------------

    private Func<Dictionary<string, object?>> RouteFailover(WorkflowCanvas canvas, StackPanel form, JsonElement saved, bool hasSource, bool isSaved,
        string[] ids, string[] models, string? oldModel)
    {
        string source;
        List<string> fallbacks;
        if (hasSource)
        {
            source = Json.Text(saved, "source");
            fallbacks = RouteStrings(saved, "fallbacks").ToList();
        }
        else
        {
            if (ids.Distinct().Count() < 2) throw new InvalidOperationException(Localized.Get("故障转移至少需要两个可用模型，请先在「模型与服务」中再添加一个模型。"));
            source = ids.FirstOrDefault(id => !models.Contains(id)) ?? throw new InvalidOperationException(Localized.Get("所有模型都已配置故障转移。"));
            fallbacks = [ids.FirstOrDefault(id => id != source) ?? ids[0]];
        }
        var originalSource = source;
        var originalFallbacks = fallbacks.ToArray();
        var served = isSaved && RouteFlag(saved, "enabled", true) && saved.TryGetProperty("activity", out var activity)
            ? Json.OptionalText(activity, "served") : null;
        var servedNote = RouteText("", secondary: true, size: 12);
        void Draw()
        {
            var active = source == originalSource && fallbacks.SequenceEqual(originalFallbacks) ? served : null;
            var graph = new WorkflowGraph();
            var client = new WorkflowNode("client", Localized.Get("客户端")) { Icon = ArcIcons.Inbox, HasDefaultInput = false, State = WorkflowNodeState.Done };
            var mixin = new WorkflowNode("mixin", "Mixin") { Icon = ArcIcons.Zap, State = WorkflowNodeState.Done };
            var primary = RouteNode("source", source, Localized.Get("主模型"), ArcIcons.Cloud, active == source);
            graph.Nodes.Add(client);
            graph.Nodes.Add(mixin);
            graph.Nodes.Add(primary);
            graph.Connect(client, mixin);
            graph.Connect(mixin, primary);
            var previous = primary;
            for (var index = 0; index < fallbacks.Count; index++)
            {
                var node = RouteNode($"fallback-{index}", fallbacks[index], Localized.Format("备用 {0}", index + 1), ArcIcons.Cloud, active == fallbacks[index]);
                graph.Nodes.Add(node);
                graph.Connect(previous, node);
                previous = node;
            }
            canvas.Height = 220;
            canvas.Graph = graph;
            servedNote.Text = active != null ? Localized.Format("最近一次响应：{0}", RouteModelName(active)) : "";
        }

        var rows = new StackPanel { Spacing = 8 };
        var addFallback = new ArcButton { Content = Localized.Get("添加备用模型"), Variant = ButtonVariant.Secondary, Size = ControlSize.Small, HorizontalAlignment = HorizontalAlignment.Left };
        void Render()
        {
            rows.Children.Clear();
            rows.Children.Add(RouteModelBox(Localized.Get("原模型"), ids, source, picked => { source = picked; Draw(); }, "route-source"));
            for (var index = 0; index < fallbacks.Count; index++)
            {
                var position = index;
                var box = RouteModelBox(Localized.Format("备用模型 {0}", index + 1), ids, fallbacks[index], picked => { fallbacks[position] = picked; Draw(); }, $"route-fallback-{position}");
                void Move(int delta)
                {
                    (fallbacks[position], fallbacks[position + delta]) = (fallbacks[position + delta], fallbacks[position]);
                    draftDirty = true; Render(); Draw();
                }
                var up = new ArcButton { Content = "↑", Variant = ButtonVariant.Ghost, Size = ControlSize.Small, IsEnabled = index > 0 };
                var down = new ArcButton { Content = "↓", Variant = ButtonVariant.Ghost, Size = ControlSize.Small, IsEnabled = index < fallbacks.Count - 1 };
                var remove = new ArcButton { Content = "✕", Variant = ButtonVariant.Ghost, Size = ControlSize.Small };
                ToolTip.SetTip(up, Localized.Format("上移备用模型 {0}", index + 1));
                ToolTip.SetTip(down, Localized.Format("下移备用模型 {0}", index + 1));
                ToolTip.SetTip(remove, Localized.Format("移除备用模型 {0}", index + 1));
                up.Click += (_, _) => Move(-1);
                down.Click += (_, _) => Move(1);
                remove.Click += (_, _) => { fallbacks.RemoveAt(position); draftDirty = true; Render(); Draw(); };
                var line = new Grid { ColumnDefinitions = new ColumnDefinitions("*,Auto,Auto,Auto"), ColumnSpacing = 6 };
                foreach (var button in new Control[] { up, down, remove }) { button.VerticalAlignment = VerticalAlignment.Bottom; }
                Grid.SetColumn(up, 1); Grid.SetColumn(down, 2); Grid.SetColumn(remove, 3);
                foreach (var cell in new Control[] { box, up, down, remove }) line.Children.Add(cell);
                rows.Children.Add(line);
            }
            addFallback.IsEnabled = fallbacks.Count < RouteMaxFallbacks;
        }
        AutomationProperties.SetAutomationId(addFallback, "route-fallback-add");
        addFallback.Click += (_, _) =>
        {
            var used = fallbacks.Append(source).ToHashSet();
            fallbacks.Add(ids.FirstOrDefault(id => !used.Contains(id)) ?? ids[0]);
            draftDirty = true; Render(); Draw();
        };
        form.Children.Add(rows);
        form.Children.Add(addFallback);
        form.Children.Add(servedNote);
        form.Children.Add(RouteText(Localized.Get("429 · 5xx · 连接失败 · 超时，且尚未输出"), secondary: true, size: 12));
        Render();
        Draw();
        var name = RouteNameField(form, hasSource ? Json.OptionalText(saved, "id") ?? "" : "", true, () => source);
        if (hasSource && !isSaved && (name.Text ?? "").Length > 0 && models.Contains(RouteVirtualPrefix + name.Text)) name.Text = RouteUniqueName(name.Text!, models);
        return () =>
        {
            var id = (name.Text ?? "").Trim();
            if (id.Contains('/')) throw new InvalidOperationException(Localized.Get("名称不能包含 /。"));
            if (fallbacks.Count is < 1 or > RouteMaxFallbacks) throw new InvalidOperationException(Localized.Get("请配置 1–4 个备用模型。"));
            if (fallbacks.Contains(source) || fallbacks.Distinct().Count() != fallbacks.Count)
                throw new InvalidOperationException(Localized.Get("备用模型不能重复，也不能与原模型相同。"));
            if (id.Length == 0 && source != oldModel && models.Contains(source)) throw new InvalidOperationException(Localized.Format("{0} 已配置故障转移。", RouteModelName(source)));
            var payload = RouteSavedFields(saved, hasSource);
            payload["source"] = source;
            payload["fallbacks"] = fallbacks.ToArray();
            if (id.Length > 0) payload["id"] = id; else payload.Remove("id");
            return payload;
        };
    }
}
