using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Media;

namespace CodexMixin;

// The 模型路由 page: "route list --json" and "route models --json" read the routes
// and the routable models; "route set" and "route delete" write them. The CLI owns
// validation, storage, and applying the change; this page only edits drafts.
// Every route is edited inside its own diagram: click a model to change it.
public sealed partial class MainWindow
{
    private const string VirtualRoutePrefix = "auto/";
    private const int MaxPanels = 8;
    private const int MaxFallbacks = 4;
    private string? routeSelection;
    // Routable models of the current page load, by id: display name and provider id.
    private Dictionary<string, (string Name, string Provider)> routeModels = [];
    private Dictionary<string, JsonElement> routeProviders = [];

    private static (string Title, string Glyph, string Description) RouteKind(string kind) => kind switch
    {
        "fusion" => (Localized.Get("融合"), "\uE8F1", Localized.Get("每个新用户轮次先由 Panel 模型并行给出意见，Judge 对比汇总，再由 Final 模型修改和回复；同一轮内的工具续调直接交给 Final 模型。")),
        "schedule" => (Localized.Get("定时"), "\uE823", Localized.Get("按本机当前时间选择模型。开始时间包含、结束时间不包含；结束早于开始表示跨午夜，时段不能重叠。")),
        _ => (Localized.Get("故障转移"), "\uE8AB", Localized.Get("原模型报任何错误，或 30 秒内没有输出时，按顺序改用备用模型；最后一个模型不设超时，报错直接透传。"))
    };

    private string ModelName(string id) => routeModels.TryGetValue(id, out var model) ? model.Name : id;
    private string ModelProvider(string id) => routeModels.TryGetValue(id, out var model) ? model.Provider : "";
    private string ProviderName(string id) =>
        routeProviders.TryGetValue(id, out var provider) ? Json.OptionalText(provider, "display_name") ?? id : id.Length == 0 ? Localized.Get("其他") : id;

    private static string[] OptionalStrings(JsonElement row, string key) =>
        row.ValueKind == JsonValueKind.Object && row.TryGetProperty(key, out var value) && value.ValueKind == JsonValueKind.Array ? Json.Strings(value) : [];

    private static long OptionalNumber(JsonElement row, string key, long fallback) =>
        row.ValueKind == JsonValueKind.Object ? Json.OptionalInteger(row, key) ?? fallback : fallback;

    private static bool OptionalFlag(JsonElement row, string key, bool fallback) =>
        row.ValueKind == JsonValueKind.Object && row.TryGetProperty(key, out var value) && value.ValueKind is JsonValueKind.True or JsonValueKind.False
            ? value.GetBoolean() : fallback;

    private static int LocalMinute() => DateTime.Now.Hour * 60 + DateTime.Now.Minute;

    private static bool Covers(int start, int end, int minute) =>
        start < end ? minute >= start && minute < end : minute >= start || minute < end;

    private static string ActiveScheduleModel(JsonElement route)
    {
        var minute = LocalMinute();
        foreach (var slot in Json.Rows(Json.Field(route, "time_routes")))
            if (Covers((int)Json.Integer(slot, "start_minute"), (int)Json.Integer(slot, "end_minute"), minute)) return Json.Text(slot, "model");
        return Json.Text(route, "final_model");
    }

    /// Referenced models in the order the route reaches them: the model that
    /// answers comes first, so its provider leads the logo stack.
    private static string[] RoutePrecedence(JsonElement route) => Json.Text(route, "kind") switch
    {
        "fusion" => [Json.Text(route, "final_model"), Json.Text(route, "judge_model"), .. OptionalStrings(route, "panel_models")],
        "schedule" => [ActiveScheduleModel(route), .. Json.Rows(Json.Field(route, "time_routes")).Select(slot => Json.Text(slot, "model")), Json.Text(route, "final_model")],
        _ => [Json.Text(route, "source"), .. OptionalStrings(route, "fallbacks")]
    };

    private string RouteSummary(JsonElement route) => Json.Text(route, "kind") switch
    {
        "fusion" => Localized.Format("{0} 个 Panel → Judge {1} → Final {2}", OptionalStrings(route, "panel_models").Length, ModelName(Json.Text(route, "judge_model")), ModelName(Json.Text(route, "final_model"))),
        "schedule" => Localized.Format("当前 {0} · {1} 个时段", ModelName(ActiveScheduleModel(route)), Json.Rows(Json.Field(route, "time_routes")).Length),
        _ => $"{ModelName(Json.Text(route, "source"))} → " + string.Join(" → ", OptionalStrings(route, "fallbacks").Select(ModelName))
    };

    private string RouteTitle(JsonElement route) =>
        Json.Text(route, "kind") == "failover" && string.IsNullOrEmpty(Json.OptionalText(route, "id"))
            ? ModelName(Json.Text(route, "source")) : Json.Text(route, "model");

    private FrameworkElement ProviderLogo(string providerId, double size) =>
        routeProviders.TryGetValue(providerId, out var provider) ? Marks.Provider(provider, size)
            : new FontIcon { Glyph = "\uE950", FontSize = size * 0.8 };

    /// Overlapping provider logos by precedence (at most three), with the route kind as a badge.
    private FrameworkElement RouteIcon(string kind, IEnumerable<string> models)
    {
        var providers = models.Select(ModelProvider).Where(id => id.Length > 0).Distinct().ToArray();
        var shown = providers.Take(3).ToArray();
        var host = new Grid { Width = 44, Height = 34, VerticalAlignment = VerticalAlignment.Center };
        if (shown.Length == 0)
            host.Children.Add(new Border { Style = Themed("LogoChip"), Width = 26, Height = 26, HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Top,
                Child = new FontIcon { Glyph = RouteKind(kind).Glyph, FontSize = 13 } });
        // The leading provider is drawn last so it sits on top.
        for (var index = shown.Length - 1; index >= 0; index--)
            host.Children.Add(new Border
            {
                Style = Themed("LogoChip"), Width = 24, Height = 24, Child = ProviderLogo(shown[index], 16),
                HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Top, Margin = new Thickness(index * 8, index * 3, 0, 0)
            });
        var badge = new Border
        {
            Style = Themed("RouteBadge"), Width = 16, Height = 16, HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Bottom,
            Child = new FontIcon { Glyph = RouteKind(kind).Glyph, FontSize = 8 }
        };
        host.Children.Add(badge);
        if (providers.Length > shown.Length)
            ToolTipService.SetToolTip(host, string.Join("、", providers.Select(ProviderName)));
        return host;
    }

    private async Task RoutingAsync(CancellationToken token)
    {
        var snapshots = await Task.WhenAll(
            Client.QueryAsync(["route", "list", "--json"], token),
            Client.QueryAsync(["route", "models", "--json"], token),
            Client.QueryAsync(["providers", "list", "--json"], token));
        var routes = Json.Rows(Json.Field(snapshots[0], "routes"));
        var options = Json.Rows(Json.Field(snapshots[1], "models"))
            .Where(row => !Json.Text(row, "id").StartsWith(VirtualRoutePrefix, StringComparison.Ordinal)).ToArray();
        var ids = options.Select(row => Json.Text(row, "id")).Distinct().ToArray();
        routeModels = options.GroupBy(row => Json.Text(row, "id")).ToDictionary(group => group.Key,
            group => (Json.OptionalText(group.First(), "display_name") ?? group.Key, Json.OptionalText(group.First(), "provider_id") ?? ""));
        routeProviders = Json.Rows(Json.Field(snapshots[2], "providers")).GroupBy(row => Json.Text(row, "id")).ToDictionary(group => group.Key, group => group.First());
        ApplyChanges.Content = Localized.Get("保存并重启网关");
        if (ids.Length == 0)
        {
            PageBody.Children.Add(Card(Localized.Get("暂无可用模型"), Localized.Get("先在「模型与服务」中添加供应商并选择模型，再配置模型路由。")));
            return;
        }
        var models = routes.Select(route => Json.Text(route, "model")).ToArray();
        var editor = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch, VerticalContentAlignment = VerticalAlignment.Top };
        var list = new ListView { SelectionMode = SelectionMode.Single };
        AutomationProperties.SetAutomationId(list, "route-list");
        foreach (var route in routes)
        {
            var item = new ListViewItem { Tag = Json.Text(route, "model"), Content = RouteRow(route) };
            AutomationProperties.SetName(item, Json.Text(route, "model"));
            ToolTipService.SetToolTip(item, $"{RouteKind(Json.Text(route, "kind")).Title} · {RouteSummary(route)}");
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
            ShowRouteEditor(editor, route, true, Json.Text(route, "kind"), ids, models);
        }
        async Task Draft(string kind, JsonElement? template)
        {
            try
            {
                if (!await ConfirmDiscardAsync(Localized.Get("新建路由"))) return;
                Select(null);
                ShowRouteEditor(editor, template, false, kind, ids, models);
                MarkDirty();
            }
            catch (Exception error) { ShowNotice(Localized.Get("新建路由失败"), error.Message, InfoBarSeverity.Error); }
        }
        list.SelectionChanged += async (_, _) =>
        {
            if (selecting || list.SelectedItem is not ListViewItem item || ReferenceEquals(item, current)) return;
            if (!await ConfirmDiscardAsync(Localized.Get("切换路由"))) { Select(current); return; }
            current = item;
            Open(item);
        };

        var newMenu = new ContextMenu();
        foreach (var kind in new[] { "fusion", "schedule", "failover" })
        {
            var (title, _, description) = RouteKind(kind);
            var item = new MenuItem { Header = Localized.Format("新建{0}", title) };
            AutomationProperties.SetAutomationId(item, $"route-new-{kind}");
            ToolTipService.SetToolTip(item, description);
            item.Click += async (_, _) => await Draft(kind, null);
            newMenu.Items.Add(item);
        }
        var create = new Button { Content = IconLabel("\uE710", Localized.Get("新建")) };
        Menus.Attach(create, newMenu);
        AutomationProperties.SetAutomationId(create, "route-new");
        AutomationProperties.SetName(create, Localized.Get("新建路由"));
        var sideHeader = new Grid();
        sideHeader.Children.Add(new TextBlock { Text = Localized.Get("路由"), Style = Themed("BodyStrongTextBlockStyle"), FontSize = 16, VerticalAlignment = VerticalAlignment.Center });
        create.HorizontalAlignment = HorizontalAlignment.Right;
        sideHeader.Children.Add(create);
        var side = new Stack { Spacing = 8 };
        side.Children.Add(sideHeader);
        side.Children.Add(routes.Length == 0 ? new TextBlock { Text = Localized.Get("还没有模型路由。"), Style = Themed("SecondaryText") } : list);

        // Suggestions are not running until saved, so each stays one quiet line.
        var suggestions = RouteSuggestions(ids, routes);
        if (suggestions.Length > 0)
        {
            side.Children.Add(new TextBlock { Text = Localized.Get("推荐路由"), Style = Themed("ColumnHeader"), Margin = new Thickness(4, 12, 0, 0) });
            foreach (var suggestion in suggestions)
            {
                var kind = Json.Text(suggestion, "kind");
                var row = new SpacedGrid { ColumnSpacing = 10, Margin = new Thickness(4, 2, 0, 2) };
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                var icon = RouteIcon(kind, RoutePrecedence(suggestion));
                icon.Opacity = 0.6;
                row.Children.Add(icon);
                var name = new TextBlock { Text = VirtualRoutePrefix + Json.Text(suggestion, "id"), Style = Themed("Caption"), FontSize = 13, VerticalAlignment = VerticalAlignment.Center };
                ToolTipService.SetToolTip(name, $"{RouteKind(kind).Title} · {RouteSummary(suggestion)}");
                Grid.SetColumn(name, 1); row.Children.Add(name);
                var enable = new Button { Content = Localized.Get("启用"), Padding = new Thickness(10, 3, 10, 4), FontSize = 12, VerticalAlignment = VerticalAlignment.Center };
                AutomationProperties.SetName(enable, Localized.Format("启用 {0}", name.Text));
                enable.Click += async (_, _) => await Draft(kind, suggestion);
                Grid.SetColumn(enable, 2); row.Children.Add(enable);
                side.Children.Add(row);
            }
        }

        var layout = new SpacedGrid { ColumnSpacing = 16 };
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(300) });
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        layout.Children.Add(new Border { Style = Themed("Card"), Padding = new Thickness(12), VerticalAlignment = VerticalAlignment.Top, Child = side });
        Grid.SetColumn(editor, 1);
        layout.Children.Add(editor);
        PageBody.Children.Add(layout);
        var initial = list.Items.Cast<ListViewItem>().FirstOrDefault(item => (string)item.Tag == routeSelection)
            ?? list.Items.Cast<ListViewItem>().FirstOrDefault();
        if (initial == null) editor.Content = Card(Localized.Get("新建模型路由"), Localized.Get("融合、定时与故障转移都在这里配置，保存后网关自动重启。"));
        else { Select(initial); Open(initial); }
    }

    private UIElement RouteRow(JsonElement route)
    {
        var row = new SpacedGrid { ColumnSpacing = 10, Margin = new Thickness(0, 6, 0, 6) };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.Children.Add(RouteIcon(Json.Text(route, "kind"), RoutePrecedence(route)));
        var name = new TextBlock { Text = RouteTitle(route), TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center };
        if (!OptionalFlag(route, "enabled", true)) name.Style = Themed("Caption");
        Grid.SetColumn(name, 1);
        row.Children.Add(name);
        return row;
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
        var chain = ids.GroupBy(ModelProvider).Select(group => group.First()).ToArray();
        if (chain.Length > 1)
            drafts.Add(new() { ["kind"] = "failover", ["id"] = "never-stuck", ["source"] = chain[0], ["fallbacks"] = chain.Skip(1).Take(MaxFallbacks).ToArray() });
        foreach (var group in ids.GroupBy(id => ModelName(id).ToLowerInvariant()).OrderBy(group => group.Key, StringComparer.Ordinal))
        {
            var same = group.ToArray();
            if (same.Select(ModelProvider).Distinct().Count() < 2) continue;
            drafts.Add(new() { ["kind"] = "failover", ["id"] = group.Key.Replace('/', '-').Replace(' ', '-'), ["source"] = same[0], ["fallbacks"] = same.Skip(1).Take(MaxFallbacks).ToArray() });
        }
        var taken = routes.Select(route => Json.Text(route, "model")).ToHashSet();
        return drafts.Where(draft => !taken.Contains(VirtualRoutePrefix + (string)draft["id"]!))
            .Select(draft => JsonSerializer.SerializeToElement(draft)).ToArray();
    }

    // Builds the draft editor for one route. `route` is the saved route, or a
    // suggestion template when `isSaved` is false. Saved fields this page does
    // not edit are carried over so a save never drops them.
    private void ShowRouteEditor(ContentControl editor, JsonElement? route, bool isSaved, string kind, string[] ids, string[] models)
    {
        var saved = route ?? default;
        var oldModel = isSaved ? Json.Text(saved, "model") : null;
        var (title, _, description) = RouteKind(kind);
        var body = new Stack { Spacing = 18 };
        var header = new SpacedGrid { ColumnSpacing = 12 };
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        header.Children.Add(RouteIcon(kind, route is { } shown ? RoutePrecedence(shown) : []));
        var heading = new Stack { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        heading.Children.Add(new TextBlock { Text = isSaved ? RouteTitle(saved) : Localized.Format("新建{0}", title), Style = Themed("BodyStrongTextBlockStyle"), FontSize = 18, TextTrimming = TextTrimming.CharacterEllipsis });
        var kindLabel = new TextBlock { Text = title, Style = Themed("Caption") };
        ToolTipService.SetToolTip(kindLabel, description);
        heading.Children.Add(kindLabel);
        Grid.SetColumn(heading, 1);
        header.Children.Add(heading);
        if (oldModel != null)
        {
            var menu = new ContextMenu();
            var delete = new MenuItem { Header = Localized.Get("删除路由…"), Icon = new FontIcon { Glyph = "\uE74D", FontSize = 14 } };
            AutomationProperties.SetAutomationId(delete, "route-delete");
            delete.Click += async (_, _) =>
            {
                try
                {
                    if (!await ConfirmAsync(Localized.Get("删除模型路由？"), Localized.Format("将删除 {0}，保存后网关自动重启。此操作不可撤销。", oldModel), Localized.Get("删除"))) return;
                    routeSelection = null;
                    await ChangeAsync(Localized.Get("删除模型路由"), ["route", "delete", "--model", oldModel], apply: true);
                }
                catch (Exception error) { ShowNotice(Localized.Get("删除模型路由失败"), error.Message, InfoBarSeverity.Error); }
            };
            menu.Items.Add(delete);
            var more = new Button { Content = new FontIcon { Glyph = "\uE712", FontSize = 14 }, Style = Themed("FlatButton"), VerticalAlignment = VerticalAlignment.Top };
            Menus.Attach(more, menu);
            AutomationProperties.SetAutomationId(more, "route-more");
            AutomationProperties.SetName(more, Localized.Get("更多"));
            ToolTipService.SetToolTip(more, Localized.Get("更多"));
            Grid.SetColumn(more, 2);
            header.Children.Add(more);
        }
        body.Children.Add(header);
        var flow = new Stack { Spacing = 8 };
        var form = new Stack { Spacing = 12 };
        body.Children.Add(flow);
        body.Children.Add(form);
        var build = kind switch
        {
            "fusion" => FusionEditor(flow, form, saved, route != null, isSaved, ids, models),
            "schedule" => ScheduleEditor(flow, form, saved, route != null, isSaved, ids, models),
            _ => FailoverEditor(flow, form, saved, route != null, isSaved, ids, models, oldModel)
        };
        pendingApply = async () =>
        {
            var payload = build();
            payload.Remove("model");
            payload.Remove("activity");
            payload["kind"] = kind;
            var arguments = new List<string> { "route", "set", "--route-json", JsonSerializer.Serialize(payload) };
            if (oldModel != null) arguments.AddRange(["--replace-model", oldModel]);
            routeSelection = payload.TryGetValue("id", out var id) && id is string name && name.Length > 0 ? VirtualRoutePrefix + name : (string?)payload["source"];
            await ChangeAsync(Localized.Get("保存模型路由"), arguments.ToArray(), apply: true);
        };
        editor.Content = new Border { Style = Themed("Card"), Child = body };
    }

    private static Dictionary<string, object?> SavedFields(JsonElement saved, bool hasSource) =>
        hasSource ? JsonSerializer.Deserialize<Dictionary<string, object?>>(saved.GetRawText())! : new Dictionary<string, object?>();

    private static string UniqueRouteName(string stem, string[] models)
    {
        for (var index = 1; ; index++)
        {
            var name = index == 1 ? stem : $"{stem}-{index}";
            if (!models.Contains(VirtualRoutePrefix + name)) return name;
        }
    }

    /// The name field and the model Codex selects for it.
    private TextBox RouteNameField(Stack form, string value, bool optional, Func<string> fallback)
    {
        var name = Input(Localized.Get("名称"), value);
        if (optional) Placeholder.Set(name, Localized.Get("可选；留空则直接接管原模型"));
        AutomationProperties.SetAutomationId(name, "route-name");
        var entry = new TextBlock { Style = Themed("SecondaryText") };
        void ShowEntry()
        {
            var text = name.Text.Trim();
            entry.Text = Localized.Format("Codex 中选择的模型：{0}", (text.Length == 0 && optional ? fallback() : VirtualRoutePrefix + text));
        }
        name.TextChanged += (_, _) => { ShowEntry(); MarkDirty(); };
        ShowEntry();
        form.Children.Add(Field.Wrap(name));
        form.Children.Add(entry);
        return name;
    }

    private static string RouteName(TextBox name)
    {
        var value = name.Text.Trim();
        if (value.Length == 0 || value.Contains('/')) throw new InvalidOperationException(Localized.Get("名称不能为空，也不能包含 /。"));
        return value;
    }

    // ---- Diagram pieces ------------------------------------------------------

    private static TextBlock StageHeader(string text) => new() { Text = text, Style = Themed("ColumnHeader"), Margin = new Thickness(2, 0, 0, 2) };

    private static FontIcon FlowArrow(string glyph, bool active = false, string? activeHint = null)
    {
        var arrow = new FontIcon { Glyph = glyph, Style = Themed(active ? "FlowArrowActive" : "FlowArrow"), HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center };
        if (active) ToolTipService.SetToolTip(arrow, Localized.Get(activeHint ?? "当前时段"));
        return arrow;
    }

    // Connect the actual laid-out controls, so curves follow wrapping and resize.
    private sealed class RouteLinks : FrameworkElement
    {
        private readonly (FrameworkElement From, FrameworkElement To, bool Active)[] links;
        private static readonly DependencyProperty LineBrushProperty = DependencyProperty.Register(
            "LineBrush", typeof(Brush), typeof(RouteLinks), new FrameworkPropertyMetadata(null, FrameworkPropertyMetadataOptions.AffectsRender));
        private static readonly DependencyProperty ActiveBrushProperty = DependencyProperty.Register(
            "ActiveBrush", typeof(Brush), typeof(RouteLinks), new FrameworkPropertyMetadata(null, FrameworkPropertyMetadataOptions.AffectsRender));

        internal RouteLinks(IEnumerable<(FrameworkElement From, FrameworkElement To, bool Active)> connections)
        {
            links = connections.ToArray();
            IsHitTestVisible = false;
            SetResourceReference(LineBrushProperty, "TextFillColorTertiaryBrush");
            SetResourceReference(ActiveBrushProperty, "AccentTextFillColorPrimaryBrush");
            foreach (var item in links.SelectMany(link => new[] { link.From, link.To }).Distinct())
            {
                item.SizeChanged += (_, _) => InvalidateVisual();
                item.Loaded += (_, _) => InvalidateVisual();
            }
        }

        protected override void OnRender(DrawingContext context)
        {
            base.OnRender(context);
            foreach (var (from, to, active) in links)
            {
                if (!from.IsArrangeValid || !to.IsArrangeValid || from.ActualWidth <= 0 || to.ActualWidth <= 0) continue;
                var start = from.TranslatePoint(new Point(from.ActualWidth, from.ActualHeight / 2), this);
                var end = to.TranslatePoint(new Point(0, to.ActualHeight / 2), this);
                var brush = (Brush?)GetValue(active ? ActiveBrushProperty : LineBrushProperty);
                if (brush == null || end.X <= start.X) continue;
                var pen = new Pen(brush, active ? 1.6 : 1);
                var dx = end.X - start.X;
                var path = new StreamGeometry();
                using (var geometry = path.Open())
                {
                    geometry.BeginFigure(start, false, false);
                    geometry.BezierTo(new Point(start.X + dx * 0.6, start.Y), new Point(end.X - dx * 0.6, end.Y), end, true, false);
                    geometry.BeginFigure(new Point(end.X - 5, end.Y - 3), false, false);
                    geometry.LineTo(end, true, false);
                    geometry.LineTo(new Point(end.X - 5, end.Y + 3), true, false);
                }
                context.DrawGeometry(null, pen, path);
            }
        }
    }

    /// A compact clickable model node, matching the shared route diagrams.
    private Button ModelNode(string id, string[] ids, Action<Button> onClick, string? automationId = null)
    {
        var content = new SpacedGrid { ColumnSpacing = 7 };
        content.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        content.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        content.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        content.Children.Add(ProviderLogo(ModelProvider(id), 16));
        var available = ids.Contains(id);
        var text = new TextBlock { Text = ModelName(id), TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center };
        if (!available) text.Style = Themed("CriticalCaption");
        Grid.SetColumn(text, 1);
        content.Children.Add(text);
        var chevron = new FontIcon { Glyph = "\uE70D", FontSize = 10 };
        chevron.SetResourceReference(Control.ForegroundProperty, "TextFillColorTertiaryBrush");
        Grid.SetColumn(chevron, 2); content.Children.Add(chevron);
        var node = new Button
        {
            Content = content, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Padding = new Thickness(10, 4, 10, 4), MinWidth = 0, Height = 44
        };
        AutomationProperties.SetName(node, ModelName(id));
        if (automationId != null) AutomationProperties.SetAutomationId(node, automationId);
        ToolTipService.SetToolTip(node, Localized.Format("{0} · {1}\n点击更换模型", id, available ? ProviderName(ModelProvider(id)) : Localized.Get("模型不可用")));
        node.Click += (_, _) => onClick(node);
        return node;
    }

    private static Button AddNode(string label, Action<Button> onClick)
    {
        var button = new Button
        {
            Content = IconLabel("\uE710", label), Style = Themed("FlatButton"), HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Left, Padding = new Thickness(10, 6, 10, 6)
        };
        AutomationProperties.SetName(button, label);
        button.Click += (_, _) => onClick(button);
        return button;
    }

    private static Button IconButton(string glyph, string name, Action onClick, bool enabled = true)
    {
        var button = new Button { Content = new FontIcon { Glyph = glyph, FontSize = 12 }, Style = Themed("FlatButton"), Padding = new Thickness(8), IsEnabled = enabled, VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetName(button, name);
        ToolTipService.SetToolTip(button, name);
        button.Click += (_, _) => onClick();
        return button;
    }

    /// Model picker anchored to a node: a standard search box over the models,
    /// grouped by provider. Panels use checkboxes; single choices close on pick.
    private void PickModels(FrameworkElement anchor, string title, string[] ids, IReadOnlyList<string> selected, bool multi, Action<string[]> changed, Action closed)
    {
        var chosen = selected.ToList();
        var flyout = new Popup { PlacementTarget = anchor, Placement = PlacementMode.Bottom, StaysOpen = false, AllowsTransparency = true };
        var search = new TextBox();
        Placeholder.Set(search, Localized.Get("搜索模型"));
        AutomationProperties.SetAutomationId(search, "route-model-search");
        AutomationProperties.SetName(search, Localized.Get("搜索模型"));
        var caption = new TextBlock { Style = Themed("Caption") };
        var items = new Stack { Spacing = 2 };
        var boxes = new List<(CheckBox Box, string Id)>();
        void Count()
        {
            caption.Text = multi ? Localized.Format("已选 {0} / {1}", chosen.Count, MaxPanels) : "";
            foreach (var (box, id) in boxes) box.IsEnabled = chosen.Contains(id) || chosen.Count < MaxPanels;
        }
        void Fill()
        {
            items.Children.Clear(); boxes.Clear();
            var query = search.Text.Trim();
            var shown = ids.Where(id => query.Length == 0 || id.Contains(query, StringComparison.OrdinalIgnoreCase)
                || ModelName(id).Contains(query, StringComparison.OrdinalIgnoreCase)
                || ProviderName(ModelProvider(id)).Contains(query, StringComparison.OrdinalIgnoreCase)).ToArray();
            foreach (var group in shown.GroupBy(ModelProvider))
            {
                var heading = new Stack { Orientation = Orientation.Horizontal, Spacing = 6, Margin = new Thickness(4, 8, 0, 2) };
                heading.Children.Add(ProviderLogo(group.Key, 14));
                heading.Children.Add(new TextBlock { Text = ProviderName(group.Key), Style = Themed("ColumnHeader") });
                items.Children.Add(heading);
                foreach (var id in group)
                {
                    var label = new Stack();
                    label.Children.Add(new TextBlock { Text = ModelName(id), TextTrimming = TextTrimming.CharacterEllipsis });
                    if (ModelName(id) != id) label.Children.Add(new TextBlock { Text = id, Style = Themed("Mono") });
                    if (multi)
                    {
                        var box = new CheckBox { Content = label, IsChecked = chosen.Contains(id), Margin = new Thickness(4, 0, 0, 0) };
                        AutomationProperties.SetName(box, ModelName(id));
                        box.Click += (_, _) =>
                        {
                            if (box.IsChecked == true) { if (!chosen.Contains(id)) chosen.Add(id); }
                            else chosen.Remove(id);
                            changed(chosen.ToArray());
                            Count();
                        };
                        boxes.Add((box, id));
                        items.Children.Add(box);
                    }
                    else
                    {
                        var row = new SpacedGrid { ColumnSpacing = 8 };
                        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                        row.Children.Add(label);
                        if (chosen.Contains(id))
                        {
                            var check = new FontIcon { Glyph = "\uE73E", FontSize = 12, VerticalAlignment = VerticalAlignment.Center };
                            Grid.SetColumn(check, 1); row.Children.Add(check);
                        }
                        var item = new Button { Content = row, Style = Themed("MenuRow") };
                        AutomationProperties.SetName(item, ModelName(id));
                        item.Click += (_, _) => { changed([id]); flyout.IsOpen = false; };
                        items.Children.Add(item);
                    }
                }
            }
            if (shown.Length == 0) items.Children.Add(new TextBlock { Text = Localized.Get("没有匹配的模型"), Style = Themed("Caption"), Margin = new Thickness(4, 8, 0, 8) });
            Count();
        }
        search.TextChanged += (_, _) => Fill();
        var content = new Stack { Spacing = 8, Width = 320 };
        var top = new Grid();
        top.Children.Add(new TextBlock { Text = title, Style = Themed("BodyStrongTextBlockStyle") });
        caption.HorizontalAlignment = HorizontalAlignment.Right; caption.VerticalAlignment = VerticalAlignment.Center;
        top.Children.Add(caption);
        content.Children.Add(top);
        content.Children.Add(search);
        content.Children.Add(new ScrollViewer { MaxHeight = 360, Content = items, Padding = new Thickness(0, 0, 12, 0) });
        var surface = new Border { Child = content, Padding = new Thickness(12), Margin = new Thickness(0, 4, 0, 0), BorderThickness = new Thickness(1), CornerRadius = new CornerRadius(8) };
        surface.SetResourceReference(Border.BackgroundProperty, "SolidBackgroundFillColorQuarternaryBrush");
        surface.SetResourceReference(Border.BorderBrushProperty, "SurfaceStrokeColorFlyoutBrush");
        flyout.Child = surface;
        flyout.Closed += (_, _) => closed();
        Fill();
        flyout.IsOpen = true;
        search.Focus();
    }

    // ---- Fusion: Panel (parallel) → Judge → Final -----------------------------

    private Func<Dictionary<string, object?>> FusionEditor(Stack flow, Stack form, JsonElement saved, bool hasSource, bool isSaved,
        string[] ids, string[] models)
    {
        var defaults = ids.Take(3).ToArray();
        var panels = (hasSource ? OptionalStrings(saved, "panel_models") : defaults).ToList();
        var judge = hasSource ? Json.Text(saved, "judge_model") : defaults[0];
        var final = hasSource ? Json.Text(saved, "final_model") : defaults.ElementAtOrDefault(1) ?? defaults[0];
        void Render()
        {
            flow.Children.Clear();
            var grid = new SpacedGrid { ColumnSpacing = 0, RowSpacing = 8 };
            foreach (var width in new[] { 1.0, 0, 1, 0, 1 })
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = width == 0 ? new GridLength(40) : new GridLength(width, GridUnitType.Star) });
            grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            void Place(FrameworkElement element, int column, int row) { Grid.SetColumn(element, column); Grid.SetRow(element, row); grid.Children.Add(element); }
            Place(StageHeader(Localized.Get("1  Panel 并行作答")), 0, 0);
            Place(StageHeader(Localized.Get("2  Judge 评审汇总")), 2, 0);
            Place(StageHeader(Localized.Get("3  Final 修改与回复")), 4, 0);
            void EditPanels(Button anchor) => PickModels(anchor, Localized.Get("Panel 模型"), ids, panels, true,
                picked => { panels = picked.ToList(); MarkDirty(); }, Render);
            var column = new Stack { Spacing = 6 };
            var panelNodes = panels.Select(panel => ModelNode(panel, ids, EditPanels)).ToArray();
            foreach (var panel in panelNodes) column.Children.Add(panel);
            var add = AddNode(panels.Count == 0 ? Localized.Get("选择 Panel 模型") : Localized.Get("编辑 Panel 模型"), EditPanels);
            AutomationProperties.SetAutomationId(add, "route-panels");
            column.Children.Add(add);
            Place(column, 0, 1);
            var judgeNode = ModelNode(judge, ids, anchor => PickModels(anchor, Localized.Get("Judge 模型"), ids, [judge], false,
                picked => { judge = picked[0]; MarkDirty(); }, Render), "route-judge");
            judgeNode.VerticalAlignment = VerticalAlignment.Center;
            Place(judgeNode, 2, 1);
            var finalNode = ModelNode(final, ids, anchor => PickModels(anchor, Localized.Get("Final 模型"), ids, [final], false,
                picked => { final = picked[0]; MarkDirty(); }, Render), "route-final");
            finalNode.VerticalAlignment = VerticalAlignment.Center;
            Place(finalNode, 4, 1);
            var links = panelNodes.Select(node => ((FrameworkElement)node, (FrameworkElement)judgeNode, false))
                .Append((judgeNode, finalNode, false));
            var connectors = new RouteLinks(links);
            Grid.SetColumnSpan(connectors, 5); Grid.SetRowSpan(connectors, 2); grid.Children.Add(connectors);
            flow.Children.Add(grid);
        }
        Render();
        var name = RouteNameField(form, hasSource ? Json.OptionalText(saved, "id") ?? "fusion" : UniqueRouteName("fusion", models), false, () => "");
        if (hasSource && !isSaved && models.Contains(VirtualRoutePrefix + name.Text)) name.Text = UniqueRouteName(name.Text, models);
        var panelTools = hasSource && saved.TryGetProperty("panel_tools", out var tools) && tools.ValueKind == JsonValueKind.Object ? tools : default;
        var minimum = Field.Labeled(new NumberBox { Minimum = 1, Maximum = MaxPanels, Value = OptionalNumber(saved, "min_successful", 1) }, Localized.Get("最少成功 Panel"));
        var timeout = Field.Labeled(new NumberBox { Minimum = 1, Value = OptionalNumber(saved, "timeout_ms", 300_000) / 1000.0 }, Localized.Get("单模型超时（秒）"));
        var intermediate = new CheckBox { Content = Localized.Get("在回答中显示 Panel / Judge 中间结果"), IsChecked = OptionalFlag(saved, "show_intermediate_results", true) };
        var readOnlyTools = new CheckBox { Content = Localized.Get("允许 Panel 使用进程内只读工具"), IsChecked = OptionalFlag(panelTools, "enabled", true) };
        var advanced = new Stack { Spacing = 12 };
        foreach (var element in new FrameworkElement[] { minimum, timeout, intermediate, readOnlyTools }) { advanced.Children.Add(Field.Wrap(element)); TrackDraft(element); }
        form.Children.Add(new Expander { Header = Localized.Get("高级选项"), Content = advanced, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch });
        return () =>
        {
            var id = RouteName(name);
            if (panels.Count is < 1 or > MaxPanels) throw new InvalidOperationException(Localized.Get("请选择 1–8 个 Panel 模型。"));
            if (!double.IsFinite(minimum.Value) || minimum.Value < 1 || minimum.Value > panels.Count || minimum.Value != Math.Truncate(minimum.Value))
                throw new InvalidOperationException(Localized.Get("最少成功 Panel 必须是 1 到 Panel 数量之间的整数。"));
            if (!double.IsFinite(timeout.Value) || timeout.Value <= 0) throw new InvalidOperationException(Localized.Get("单模型超时必须大于 0。"));
            var payload = SavedFields(saved, hasSource);
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

    // ---- Schedule: time slots → model, then the default ------------------------

    private Func<Dictionary<string, object?>> ScheduleEditor(Stack flow, Stack form, JsonElement saved, bool hasSource, bool isSaved,
        string[] ids, string[] models)
    {
        var fallback = hasSource ? Json.Text(saved, "final_model") : ids[0];
        var slots = hasSource
            ? Json.Rows(Json.Field(saved, "time_routes")).Select(slot => (Start: (int)Json.Integer(slot, "start_minute"), End: (int)Json.Integer(slot, "end_minute"), Model: Json.Text(slot, "model"))).ToList()
            : [(540, 1080, ids.ElementAtOrDefault(1) ?? ids[0])];
        void Render()
        {
            flow.Children.Clear();
            var grid = new SpacedGrid { ColumnSpacing = 10, RowSpacing = 6 };
            foreach (var width in new[] { 0, 0, 0, 0, 1.0, 0 })
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = width == 0 ? GridLength.Auto : new GridLength(width, GridUnitType.Star) });
            var row = 0;
            void Place(FrameworkElement element, int column) { Grid.SetColumn(element, column); Grid.SetRow(element, row); grid.Children.Add(element); }
            grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            Place(StageHeader(Localized.Get("时段（本机时间）")), 0);
            Place(StageHeader(Localized.Get("模型")), 4);
            var minute = LocalMinute();
            for (var index = 0; index < slots.Count; index++)
            {
                var slotIndex = index;
                var slot = slots[index];
                row++;
                grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
                var begin = new TimePicker { Time = TimeSpan.FromMinutes(slot.Start) };
                var finish = new TimePicker { Time = TimeSpan.FromMinutes(slot.End) };
                AutomationProperties.SetName(begin, Localized.Format("时段 {0} 开始", index + 1)); AutomationProperties.SetName(finish, Localized.Format("时段 {0} 结束", index + 1));
                begin.TimeChanged += (_, _) => { slots[slotIndex] = slots[slotIndex] with { Start = (int)begin.Time.TotalMinutes }; MarkDirty(); };
                finish.TimeChanged += (_, _) => { slots[slotIndex] = slots[slotIndex] with { End = (int)finish.Time.TotalMinutes }; MarkDirty(); };
                Place(begin, 0);
                Place(new TextBlock { Text = "–", VerticalAlignment = VerticalAlignment.Center }, 1);
                Place(finish, 2);
                var active = slot.Start != slot.End && Covers(slot.Start, slot.End, minute);
                Place(FlowArrow("\uE72A", active), 3);
                Place(ModelNode(slot.Model, ids, anchor => PickModels(anchor, Localized.Get("时段模型"), ids, [slots[slotIndex].Model], false,
                    picked => { slots[slotIndex] = slots[slotIndex] with { Model = picked[0] }; MarkDirty(); }, Render)), 4);
                Place(IconButton("\uE711", Localized.Get("移除此时段"), () => { slots.RemoveAt(slotIndex); MarkDirty(); Render(); }), 5);
            }
            row++;
            grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            var add = AddNode(Localized.Get("添加时段"), _ =>
            {
                var start = slots.Count == 0 ? 540 : slots[^1].End;
                slots.Add((start, (start + 240) % 1440, ids[0]));
                MarkDirty(); Render();
            });
            add.IsEnabled = slots.Count < 24;
            Grid.SetColumnSpan(add, 3);
            Place(add, 0);
            row++;
            grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            var others = new TextBlock { Text = Localized.Get("其余时间"), Style = Themed("Caption"), VerticalAlignment = VerticalAlignment.Center };
            Grid.SetColumnSpan(others, 3);
            Place(others, 0);
            var idle = !slots.Any(slot => slot.Start != slot.End && Covers(slot.Start, slot.End, minute));
            Place(FlowArrow("\uE72A", idle), 3);
            Place(ModelNode(fallback, ids, anchor => PickModels(anchor, Localized.Get("默认模型"), ids, [fallback], false,
                picked => { fallback = picked[0]; MarkDirty(); }, Render), "route-default-model"), 4);
            flow.Children.Add(grid);
        }
        Render();
        var name = RouteNameField(form, hasSource ? Json.OptionalText(saved, "id") ?? "schedule" : UniqueRouteName("schedule", models), false, () => "");
        if (hasSource && !isSaved && models.Contains(VirtualRoutePrefix + name.Text)) name.Text = UniqueRouteName(name.Text, models);
        return () =>
        {
            var id = RouteName(name);
            if (slots.Count is < 1 or > 24) throw new InvalidOperationException(Localized.Get("请配置 1–24 个时段。"));
            if (slots.Any(range => range.Start == range.End)) throw new InvalidOperationException(Localized.Get("时段的开始和结束时间不能相同。"));
            if (Enumerable.Range(0, 1440).Any(minute => slots.Count(range => Covers(range.Start, range.End, minute)) > 1))
                throw new InvalidOperationException(Localized.Get("时段不能重叠。"));
            var payload = SavedFields(saved, hasSource);
            payload["id"] = id; payload["mode"] = "time_rotation";
            payload["final_model"] = fallback;
            payload["time_routes"] = slots.Select(range => new { start_minute = range.Start, end_minute = range.End, model = range.Model }).ToArray();
            if (!payload.ContainsKey("panel_models")) payload["panel_models"] = Array.Empty<string>();
            if (!payload.ContainsKey("judge_model")) payload["judge_model"] = fallback;
            return payload;
        };
    }

    // ---- Failover: source, then fallbacks in order ------------------------------

    private Func<Dictionary<string, object?>> FailoverEditor(Stack flow, Stack form, JsonElement saved, bool hasSource, bool isSaved,
        string[] ids, string[] models, string? oldModel)
    {
        string source;
        List<string> fallbacks;
        if (hasSource)
        {
            source = Json.Text(saved, "source");
            fallbacks = OptionalStrings(saved, "fallbacks").ToList();
        }
        else
        {
            if (ids.Distinct().Count() < 2) throw new InvalidOperationException(Localized.Get("故障转移至少需要两个可用模型，请先在「模型与服务」中再添加一个模型。"));
            source = ids.FirstOrDefault(id => !models.Contains(id)) ?? throw new InvalidOperationException(Localized.Get("所有模型都已配置故障转移。"));
            fallbacks = [ids.FirstOrDefault(id => id != source) ?? ids[0]];
        }
        var originalSource = source;
        var originalFallbacks = fallbacks.ToArray();
        var served = isSaved && OptionalFlag(saved, "enabled", true) && saved.TryGetProperty("activity", out var activity)
            ? Json.OptionalText(activity, "served") : null;
        bool? wideLayout = null;
        void Render()
        {
            flow.Children.Clear();
            var wide = flow.ActualWidth >= 650;
            wideLayout = wide;
            var active = source == originalSource && fallbacks.SequenceEqual(originalFallbacks) ? served : null;
            var targets = new Stack { Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
            Grid? diagram = null;
            Border? mixin = null;
            var connections = new List<(FrameworkElement From, FrameworkElement To, bool Active)>();
            if (wide)
            {
                diagram = new Grid();
                diagram.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                diagram.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(44) });
                diagram.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                var lead = new Stack { Orientation = Orientation.Horizontal, Spacing = 22, VerticalAlignment = VerticalAlignment.Center };
                Border DiagramNode(string label, UIElement icon, bool highlighted = false)
                {
                    var content = new Stack { Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
                    content.Children.Add(icon);
                    content.Children.Add(new TextBlock { Text = label, HorizontalAlignment = HorizontalAlignment.Center, FontWeight = FontWeights.SemiBold });
                    var node = new Border { Style = Themed("ListCard"), Width = 96, Height = highlighted ? 92 : 72, Child = content };
                    if (highlighted) node.SetResourceReference(Border.BorderBrushProperty, "AccentFillColorDefaultBrush");
                    return node;
                }
                var client = DiagramNode(Localized.Get("客户端"), new FontIcon { Glyph = "\uE7C3", FontSize = 20 });
                var logo = new Image { Source = new System.Windows.Media.Imaging.BitmapImage(new Uri("pack://siteoforigin:,,,/Assets/app_icon.png")), Width = 36, Height = 36 };
                mixin = DiagramNode("Mixin", logo, highlighted: true);
                lead.Children.Add(client); lead.Children.Add(mixin);
                connections.Add((client, mixin, active != null));
                diagram.Children.Add(lead);
                Grid.SetColumn(targets, 2); diagram.Children.Add(targets);
                flow.Children.Add(diagram);
            }
            else flow.Children.Add(targets);

            UIElement TargetRow(Button node, string model, string role, int? position = null)
            {
                var row = new SpacedGrid { ColumnSpacing = 8 };
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(8) });
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                var dot = new System.Windows.Shapes.Ellipse { Width = 6, Height = 6, VerticalAlignment = VerticalAlignment.Center, StrokeThickness = 1 };
                dot.SetResourceReference(System.Windows.Shapes.Shape.StrokeProperty, active == model ? "AccentTextFillColorPrimaryBrush" : "TextFillColorTertiaryBrush");
                if (active == model) dot.SetResourceReference(System.Windows.Shapes.Shape.FillProperty, "AccentTextFillColorPrimaryBrush");
                row.Children.Add(dot);
                Grid.SetColumn(node, 1); row.Children.Add(node);
                var roleText = new TextBlock { Text = role, Style = Themed("Caption"), VerticalAlignment = VerticalAlignment.Center };
                Grid.SetColumn(roleText, 2); row.Children.Add(roleText);
                if (position is { } index)
                {
                    var controls = new Stack { Orientation = Orientation.Horizontal, Spacing = 2 };
                    void Move(int delta) { (fallbacks[index], fallbacks[index + delta]) = (fallbacks[index + delta], fallbacks[index]); MarkDirty(); Render(); }
                    controls.Children.Add(IconButton("\uE70E", Localized.Format("上移备用模型 {0}", index + 1), () => Move(-1), index > 0));
                    controls.Children.Add(IconButton("\uE70D", Localized.Format("下移备用模型 {0}", index + 1), () => Move(1), index < fallbacks.Count - 1));
                    controls.Children.Add(IconButton("\uE711", Localized.Format("移除备用模型 {0}", index + 1), () => { fallbacks.RemoveAt(index); MarkDirty(); Render(); }));
                    row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                    Grid.SetColumn(controls, 3); row.Children.Add(controls);
                }
                if (mixin != null) connections.Add((mixin, row, active == model));
                return row;
            }
            var primary = ModelNode(source, ids, anchor => PickModels(anchor, Localized.Get("原模型"), ids, [source], false,
                picked => { source = picked[0]; MarkDirty(); }, Render), "route-source");
            targets.Children.Add(TargetRow(primary, source, Localized.Get("主模型")));
            for (var index = 0; index < fallbacks.Count; index++)
            {
                var position = index;
                var node = ModelNode(fallbacks[index], ids, anchor => PickModels(anchor, Localized.Format("备用模型 {0}", position + 1), ids, [fallbacks[position]], false,
                    picked => { fallbacks[position] = picked[0]; MarkDirty(); }, Render));
                targets.Children.Add(TargetRow(node, fallbacks[index], Localized.Format("备用 {0}", index + 1), index));
            }
            if (fallbacks.Count < MaxFallbacks)
                targets.Children.Add(AddNode(Localized.Get("添加备用模型"), _ =>
                {
                    var used = fallbacks.Append(source).ToHashSet();
                    fallbacks.Add(ids.FirstOrDefault(id => !used.Contains(id)) ?? ids[0]);
                    MarkDirty(); Render();
                }));
            if (diagram != null)
            {
                var links = new RouteLinks(connections);
                Grid.SetColumnSpan(links, 3); diagram.Children.Add(links);
            }
            if (active != null) flow.Children.Add(new TextBlock { Text = Localized.Format("最近一次响应：{0}", ModelName(active)), Style = Themed("Caption") });
            flow.Children.Add(new TextBlock { Text = Localized.Get("429 · 5xx · 连接失败 · 超时，且尚未输出"), Style = Themed("Caption") });
        }
        flow.SizeChanged += (_, args) => { if (wideLayout != (args.NewSize.Width >= 650)) Render(); };
        Render();
        var name = RouteNameField(form, hasSource ? Json.OptionalText(saved, "id") ?? "" : "", true, () => source);
        if (hasSource && !isSaved && name.Text.Length > 0 && models.Contains(VirtualRoutePrefix + name.Text)) name.Text = UniqueRouteName(name.Text, models);
        return () =>
        {
            var id = name.Text.Trim();
            if (id.Contains('/')) throw new InvalidOperationException(Localized.Get("名称不能包含 /。"));
            if (fallbacks.Count is < 1 or > MaxFallbacks) throw new InvalidOperationException(Localized.Get("请配置 1–4 个备用模型。"));
            if (fallbacks.Contains(source) || fallbacks.Distinct().Count() != fallbacks.Count)
                throw new InvalidOperationException(Localized.Get("备用模型不能重复，也不能与原模型相同。"));
            if (id.Length == 0 && source != oldModel && models.Contains(source)) throw new InvalidOperationException(Localized.Format("{0} 已配置故障转移。", ModelName(source)));
            var payload = SavedFields(saved, hasSource);
            payload["source"] = source;
            payload["fallbacks"] = fallbacks.ToArray();
            if (id.Length > 0) payload["id"] = id; else payload.Remove("id");
            return payload;
        };
    }
}
