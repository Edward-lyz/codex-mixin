using System.Text.Json;
using Avalonia;
using Avalonia.Automation;
using Avalonia.Controls;
using Avalonia.Layout;
using Arc = TKCode.Arc;

namespace CodexMixin;

internal sealed partial class MainWindow
{
    private async Task<Control> BuildProvidersAsync(CancellationToken token)
    {
        var contract = await cli.QueryAsync(["interface", "--json"], token);
        Json.Interface(contract);
        var catalog = Json.Field(contract, "provider_catalog");
        var providers = Json.Rows(Json.Field(await cli.QueryAsync(["provider", "list", "--json"], token), "providers"));

        var page = new StackPanel { Spacing = 12, MaxWidth = 880, HorizontalAlignment = HorizontalAlignment.Left };
        var add = new Arc.ExpandableCard
        {
            Header = Localized.Get("新增服务商"),
            Description = providers.Length == 0 ? Localized.Get("先添加一个供应商，再选择模型并接入 Agent。") : Localized.Get("连接新的模型服务"),
            IsExpanded = providers.Length == 0,
            Content = ProviderForm(catalog, null)
        };
        AutomationProperties.SetAutomationId(add, "provider-add-card");
        page.Children.Add(add);
        foreach (var provider in providers) page.Children.Add(ProviderCard(catalog, provider));
        return page;
    }

    private static string ProviderReadiness(JsonElement provider) => !Json.Flag(provider, "enabled") ? Localized.Get("已停用") :
        Json.Text(provider, "readiness") switch { "healthy" => Localized.Get("正常"), "degraded" => Localized.Get("降级"), var value => value };

    private static Arc.Input ProviderInput(string label, string text) => new() { Label = label, Text = text, HorizontalAlignment = HorizontalAlignment.Stretch };

    private static Arc.PasswordField ProviderSecret(string label) => new() { Label = label, HorizontalAlignment = HorizontalAlignment.Stretch };

    private static Arc.Select ProviderSelect(string label, IEnumerable<(string Value, string Text)> options, string selected)
    {
        var select = new Arc.Select { Label = label, HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var (value, text) in options) select.Items.Add(new Arc.SelectItem { Content = text, Value = value });
        select.SelectedValue = selected;
        return select;
    }

    private static string ProviderValue(Arc.Select select) => select.SelectedValue as string ?? "";

    private static string[] ProviderModelIds(JsonElement provider) => Json.Strings(Json.Field(provider, "selected_models"));

    private Control ProviderCard(JsonElement catalog, JsonElement provider)
    {
        var id = Json.Text(provider, "id");
        var selected = ProviderModelIds(provider);
        var cached = Json.Field(provider, "cached_models").GetArrayLength();
        var header = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        header.Children.Add(new TextBlock { Text = Json.Text(provider, "display_name"), VerticalAlignment = VerticalAlignment.Center });
        if (Json.Text(provider, "kind") == "official")
            header.Children.Add(new Arc.Badge { Content = Localized.Get("官方"), Tone = Arc.BadgeTone.Info, Size = Arc.BadgeSize.Small });
        header.Children.Add(new Arc.Badge
        {
            Content = ProviderReadiness(provider),
            Size = Arc.BadgeSize.Small,
            Tone = !Json.Flag(provider, "enabled") ? Arc.BadgeTone.Neutral : Json.Text(provider, "readiness") == "healthy" ? Arc.BadgeTone.Success : Arc.BadgeTone.Warning
        });

        var body = new StackPanel { Spacing = 16 };
        body.Children.Add(ProviderModels(provider));
        var connection = new Arc.ExpandableCard
        {
            Header = Localized.Get("连接配置"),
            Description = Localized.Get("密钥已配置时留空保留。"),
            Content = ProviderForm(catalog, provider)
        };
        AutomationProperties.SetAutomationId(connection, "provider-connection-" + id);
        body.Children.Add(connection);
        var remove = new Arc.Button { Content = Localized.Get("删除服务商"), Variant = Arc.ButtonVariant.Danger, HorizontalAlignment = HorizontalAlignment.Left };
        remove.Click += async (_, _) =>
        {
            if (await ConfirmAsync(Localized.Get("删除服务商？"), Localized.Get("将移除当前服务商及其模型配置。")))
                await RunActionAsync(["provider", "remove", id], apply: true);
        };
        AutomationProperties.SetAutomationId(remove, "provider-remove-" + id);
        body.Children.Add(remove);
        var card = new Arc.ExpandableCard
        {
            Header = header,
            Description = Localized.Format("{0} · {1}/{2} 个模型", ProviderReadiness(provider), selected.Length, cached),
            Content = body
        };
        AutomationProperties.SetAutomationId(card, "provider-card-" + id);
        return card;
    }

    private Control ProviderForm(JsonElement catalog, JsonElement? provider)
    {
        var presets = Json.Rows(Json.Field(catalog, "presets"));
        var protocols = Json.Rows(Json.Field(catalog, "protocols"));
        var form = new StackPanel { Spacing = 12 };
        var scope = provider == null ? "provider-new" : "provider-edit";
        string Stored(string key) => provider is { } row ? Json.OptionalText(row, key) ?? "" : "";

        var preset = ProviderSelect(Localized.Get("供应商类型"), presets.Select(row => (Json.Text(row, "id"), Json.Text(row, "display_name"))),
            provider is { } stored ? Json.OptionalText(stored, "preset_id") ?? "custom" : Json.Text(presets[0], "id"));
        preset.IsEnabled = provider == null;
        var id = ProviderInput(Localized.Get("供应商 ID"), provider is { } p ? Json.Text(p, "id") : "");
        id.IsEnabled = provider == null;
        var name = ProviderInput(Localized.Get("显示名称"), provider is { } n ? Json.Text(n, "display_name") : "");
        var url = ProviderInput(Localized.Get("Base URL（预设可留空）"), provider is { } u ? Json.Text(u, "base_url") : "");
        var website = ProviderInput(Localized.Get("供应商网站（可选）"), Stored("website_url"));
        var imagePath = ProviderInput(Localized.Get("图像生成路径（可选）"), Stored("image_generation_path"));
        var apiPath = ProviderInput(Localized.Get("API 路径（custom，可选）"), Stored("api_path"));
        var key = ProviderSecret(provider == null ? "API Key" : Localized.Get("新 API Key（留空保留）"));
        var protocol = ProviderSelect(Localized.Get("协议（custom）"),
            new[] { ("", Localized.Get("自动检测")) }.Concat(protocols.Select(row => (Json.Text(row, "id"), Json.Text(row, "display_name")))),
            provider is { } pr && protocols.Any(row => Json.Text(row, "id") == Json.Text(pr, "protocol")) ? Json.Text(pr, "protocol") : "");
        var username = ProviderInput(Localized.Get("OneAPI 额度用户名"), Stored("quota_username"));
        var workspace = ProviderInput("OpenCode Go workspace ID", Stored("quota_workspace_id"));
        var cookie = ProviderSecret(Localized.Get("OpenCode Go Cookie（留空保留）"));
        var accessKey = ProviderSecret(Localized.Get("AWS Access Key ID（留空保留）"));
        var secretKey = ProviderSecret(Localized.Get("AWS Secret Access Key（留空保留）"));
        var session = ProviderSecret(Localized.Get("AWS Session Token（可选）"));
        var defaultRegion = Json.Text(catalog, "default_aws_region");
        var region = ProviderInput("AWS Region", provider is { } r ? Json.OptionalText(r, "aws_region") ?? defaultRegion : defaultRegion);
        var bridge = ProviderSelect(Localized.Get("OneAPI 认证桥接"), [("disabled", "disabled"), ("ducx_loopback", "ducx_loopback")],
            provider is { } b ? Json.OptionalText(b, "baidu_auth_bridge") ?? "disabled" : "disabled");
        var report = new Arc.Checkbox
        {
            Content = Localized.Get("OneAPI 代码上报"),
            IsChecked = provider is { } br && Json.Field(br, "baidu_code_report").ValueKind == JsonValueKind.True
        };
        var oauthTimeout = ProviderInput(Localized.Get("OAuth 授权超时（秒）"), Json.Field(catalog, "oauth_timeout_seconds").ToString());
        oauthTimeout.Description = Localized.Get("将在浏览器中授权。关闭网页不会回传取消；可使用窗口的取消操作结束等待。");

        var models = provider is { } configured ? ProviderModelIds(configured) : [];
        var reviewOverride = new Arc.Checkbox
        {
            Content = Localized.Get("Codex 审查模型覆盖"),
            IsChecked = provider is { } rp && Json.OptionalText(rp, "auto_review_model") is { Length: > 0 },
            IsEnabled = models.Length > 0
        };
        var reviewModel = ProviderSelect(Localized.Get("审查模型"), models.Select(model => (model, model)),
            provider is { } rm ? Json.OptionalText(rm, "auto_review_model") ?? models.FirstOrDefault() ?? "" : "");
        var titleOverride = new Arc.Checkbox
        {
            Content = Localized.Get("Codex 会话命名模型覆盖"),
            IsChecked = provider is { } tp && Json.OptionalText(tp, "codex_thread_title_model") is { Length: > 0 },
            IsEnabled = models.Length > 0
        };
        var titleModel = ProviderSelect(Localized.Get("会话命名模型"), models.Select(model => (model, model)),
            provider is { } tm ? Json.OptionalText(tm, "codex_thread_title_model") ?? models.FirstOrDefault() ?? "" : "");

        var ducxHint = new TextBlock
        {
            Text = Localized.Get("DUCX 使用独立副本。保存时自动下载；需要登录时会打开终端显示二维码，请在终端完成扫码。关闭终端或取消后可重新登录。"),
            TextWrapping = Avalonia.Media.TextWrapping.Wrap,
            Opacity = 0.7
        };
        var ducx = new Arc.Button { Content = Localized.Get("安装 / 登录 DUCX…"), Variant = Arc.ButtonVariant.Secondary, HorizontalAlignment = HorizontalAlignment.Left };
        ducx.Click += async (_, _) => await RunActionAsync(["connect", "ducx", "--json"]);

        foreach (var control in new Control[] { preset, id, oauthTimeout, name, url, website, protocol, apiPath, imagePath, key, username, bridge, report, ducxHint, ducx,
            workspace, cookie, accessKey, secretKey, session, region })
            form.Children.Add(control);
        if (provider != null)
            foreach (var control in new Control[] { reviewOverride, reviewModel, titleOverride, titleModel }) form.Children.Add(control);

        JsonElement Definition() => presets.FirstOrDefault(row => Json.Text(row, "id") == ProviderValue(preset)) is { ValueKind: JsonValueKind.Object } found
            ? found : throw new CliFailure(Localized.Get("CLI provider catalog 包含不支持的认证方式。"));
        void Fields()
        {
            var definition = Definition();
            var authMode = Json.Text(definition, "auth_mode");
            var aws = authMode == "aws_sigv4";
            var oauth = provider == null && authMode == "local_oauth";
            oauthTimeout.IsVisible = oauth;
            id.IsVisible = !(oauth && !Json.Flag(definition, "allow_multiple"));
            key.Label = Json.Flag(definition, "api_key_required") ? "API Key"
                : authMode == "local_session" ? Localized.Get("API Key（可选；留空使用原生客户端登录状态）") : Localized.Get("API Key（可选；留空使用本地 OAuth）");
            key.IsVisible = !(aws || oauth);
            foreach (var control in new Control[] { name, url, website, imagePath }) control.IsVisible = !oauth;
            foreach (var control in new Control[] { accessKey, secretKey, session, region }) control.IsVisible = aws;
            foreach (var control in new Control[] { username, bridge, report }) control.IsVisible = Json.Flag(definition, "quota_username_required");
            foreach (var control in new Control[] { workspace, cookie }) control.IsVisible = Json.Flag(definition, "quota_credentials_required");
            protocol.IsVisible = apiPath.IsVisible = Json.Flag(definition, "custom_endpoint");
            ducxHint.IsVisible = ducx.IsVisible = bridge.IsVisible && ProviderValue(bridge) == "ducx_loopback";
        }
        void Overrides()
        {
            reviewModel.IsVisible = reviewOverride.IsChecked == true;
            titleModel.IsVisible = titleOverride.IsChecked == true;
        }
        preset.SelectionChanged += (_, _) => Fields();
        bridge.SelectionChanged += (_, _) => Fields();
        reviewOverride.IsCheckedChanged += (_, _) => Overrides();
        titleOverride.IsCheckedChanged += (_, _) => Overrides();
        Fields(); Overrides();

        foreach (var (control, suffix) in new (Control, string)[] { (preset, "preset"), (id, "id"), (name, "name"), (url, "base-url"), (protocol, "protocol"), (key, "key"), (apiPath, "api-path"), (username, "quota-username"), (bridge, "auth-bridge") })
            AutomationProperties.SetAutomationId(control, $"{scope}-{suffix}");
        foreach (var box in new TextBox[] { id, name, url, website, imagePath, apiPath, key, username, workspace, cookie, accessKey, secretKey, session, region, oauthTimeout })
            box.TextChanged += (_, _) => draftDirty = true;
        foreach (var select in new[] { preset, protocol, bridge, reviewModel, titleModel }) select.SelectionChanged += (_, _) => draftDirty = true;
        foreach (var check in new[] { report, reviewOverride, titleOverride }) check.IsCheckedChanged += (_, _) => draftDirty = true;

        var save = new Arc.Button
        {
            Content = provider == null ? Localized.Get("添加服务商") : Localized.Get("保存连接配置"),
            Variant = Arc.ButtonVariant.Primary,
            HorizontalAlignment = HorizontalAlignment.Left
        };
        save.Click += async (_, _) =>
        {
            var args = new List<string> { "provider", provider == null ? "add" : "update" };
            var definition = Definition();
            if (provider == null) args.AddRange(["--preset", ProviderValue(preset)]);
            if (provider is { } existing) args.Add(Json.Text(existing, "id"));
            else if (id.IsVisible && !string.IsNullOrWhiteSpace(id.Text)) args.AddRange(["--id", id.Text.Trim()]);
            var secrets = new Dictionary<string, string>();
            void Secret(string flag, Arc.PasswordField box, string variable)
            {
                if (string.IsNullOrEmpty(box.Text)) return;
                secrets[variable] = box.Text; args.AddRange([flag, $"@env:{variable}"]);
            }
            foreach (var (flag, box) in new[] { ("--display-name", name), ("--base-url", url), ("--website-url", website), ("--image-generation-path", imagePath) })
                if (box.IsVisible && !string.IsNullOrWhiteSpace(box.Text)) args.AddRange([flag, box.Text.Trim()]);
            if (Json.Flag(definition, "custom_endpoint"))
            {
                if (ProviderValue(protocol).Length > 0) args.AddRange(["--protocol", ProviderValue(protocol)]);
                if (!string.IsNullOrWhiteSpace(apiPath.Text)) args.AddRange(["--api-path", apiPath.Text.Trim()]);
            }
            if (Json.Text(definition, "auth_mode") == "aws_sigv4")
            {
                Secret("--aws-access-key-id", accessKey, "CM_AWS_ID");
                Secret("--aws-secret-access-key", secretKey, "CM_AWS_SECRET");
                Secret("--aws-session-token", session, "CM_AWS_SESSION");
                args.AddRange(["--aws-region", region.Text?.Trim() ?? ""]);
            }
            else if (key.IsVisible) Secret("--key", key, "CM_API_KEY");
            if (oauthTimeout.IsVisible && !string.IsNullOrWhiteSpace(oauthTimeout.Text))
                args.AddRange(["--oauth-timeout-seconds", oauthTimeout.Text.Trim()]);
            if (Json.Flag(definition, "quota_username_required"))
            {
                if (!string.IsNullOrWhiteSpace(username.Text)) args.AddRange(["--quota-username", username.Text.Trim()]);
                args.AddRange(["--baidu-auth-bridge", ProviderValue(bridge), "--baidu-code-report", report.IsChecked == true ? "true" : "false"]);
            }
            if (Json.Flag(definition, "quota_credentials_required"))
            {
                if (!string.IsNullOrWhiteSpace(workspace.Text)) args.AddRange(["--quota-workspace-id", workspace.Text.Trim()]);
                Secret("--quota-auth-cookie", cookie, "CM_QUOTA_COOKIE");
            }
            if (provider != null)
            {
                if (reviewOverride.IsChecked == true && ProviderValue(reviewModel).Length > 0) args.AddRange(["--auto-review-model", ProviderValue(reviewModel)]);
                else args.Add("--clear-auto-review-model");
                if (titleOverride.IsChecked == true && ProviderValue(titleModel).Length > 0) args.AddRange(["--codex-thread-title-model", ProviderValue(titleModel)]);
                else args.Add("--clear-codex-thread-title-model");
            }
            try { await RunActionAsync(args.ToArray(), apply: true, secrets: secrets); }
            finally
            {
                secrets.Clear();
            }
        };
        AutomationProperties.SetAutomationId(save, $"{scope}-save");
        form.Children.Add(save);
        if (provider is { } connection)
        {
            var clear = new Arc.Button { Content = Localized.Get("清除密钥"), Variant = Arc.ButtonVariant.Secondary, HorizontalAlignment = HorizontalAlignment.Left };
            clear.Click += async (_, _) => await RunActionAsync(["provider", "update", Json.Text(connection, "id"),
                Json.OptionalText(connection, "preset_id") == "aws-bedrock" ? "--clear-aws-credentials" : "--clear-key"], apply: true);
            AutomationProperties.SetAutomationId(clear, "provider-edit-clear-key");
            form.Children.Add(clear);
        }
        return form;
    }

    private Control ProviderModels(JsonElement provider)
    {
        var id = Json.Text(provider, "id");
        var models = Json.Rows(Json.Field(provider, "cached_models"));
        var selected = ProviderModelIds(provider).ToList();
        var order = Json.OptionalText(provider, "model_order") ?? "family";
        var savedContexts = Json.Field(provider, "model_context_overrides");
        var contexts = savedContexts.EnumerateObject().ToDictionary(entry => entry.Name, entry => entry.Value.GetDouble());

        var panel = new StackPanel { Spacing = 12 };
        var search = new Arc.Input { Label = Localized.Get("搜索当前服务商的模型"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var filter = ProviderSelect(Localized.Get("模型筛选"), [("all", Localized.Get("全部")), ("on", Localized.Get("已加入")), ("off", Localized.Get("未加入"))], "all");
        var orderSelect = ProviderSelect(Localized.Get("模型排序"), [("family", Localized.Get("家族与版本")), ("manual", Localized.Get("手动排序"))], order);
        orderSelect.Description = Localized.Get("手动排序时，使用已加入模型旁的上移/下移调整顺序。");
        var test = new Arc.Button { Variant = Arc.ButtonVariant.Secondary };
        var error = new TextBlock { TextWrapping = Avalonia.Media.TextWrapping.Wrap, IsVisible = false };
        var list = new StackPanel { Spacing = 4 };
        AutomationProperties.SetAutomationId(search, "provider-model-search");
        AutomationProperties.SetAutomationId(list, "model-table");
        var scroll = new ScrollViewer { Content = list, MaxHeight = 420 };

        void Count() => test.Content = Localized.Format("测试已加入的 {0} 个模型", selected.Count);
        void Rows()
        {
            list.Children.Clear();
            var query = search.Text?.Trim() ?? "";
            var mode = ProviderValue(filter);
            foreach (var model in models)
            {
                var modelId = Json.Text(model, "id");
                var on = selected.Contains(modelId);
                var label = Json.OptionalText(model, "display_name") ?? modelId;
                if (query.Length > 0 && !label.Contains(query, StringComparison.OrdinalIgnoreCase) && !modelId.Contains(query, StringComparison.OrdinalIgnoreCase)) continue;
                if (mode == "on" && !on || mode == "off" && on) continue;
                var row = new Grid { ColumnDefinitions = new ColumnDefinitions("*,Auto") };
                var check = new Arc.Checkbox
                {
                    Content = label,
                    Description = Json.OptionalText(model, "description") is { Length: > 0 } text ? $"{modelId} · {text}" : modelId,
                    IsChecked = on
                };
                AutomationProperties.SetName(check, Localized.UseEnglish ? $"Add {modelId} to Codex" : $"{modelId} 加入 Codex");
                AutomationProperties.SetAutomationId(check, "model-check-" + modelId);
                check.IsCheckedChanged += (_, _) =>
                {
                    if (check.IsChecked == true) { if (!selected.Contains(modelId)) selected.Add(modelId); }
                    else selected.Remove(modelId);
                    draftDirty = true; Count(); Rows();
                };
                row.Children.Add(check);
                if (on)
                {
                    var tools = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
                    var context = new Arc.Input
                    {
                        Label = Localized.Get("上下文 Token"), Size = Arc.ControlSize.Small, Width = 140,
                        Text = contexts.TryGetValue(modelId, out var value) && !double.IsNaN(value) ? value.ToString("0") : ""
                    };
                    AutomationProperties.SetAutomationId(context, "model-context-" + modelId);
                    context.TextChanged += (_, _) =>
                    {
                        contexts[modelId] = string.IsNullOrWhiteSpace(context.Text) ? double.NaN
                            : double.TryParse(context.Text, out var parsed) ? parsed : -1;
                        draftDirty = true;
                    };
                    tools.Children.Add(context);
                    if (ProviderValue(orderSelect) == "manual")
                    {
                        void Move(int delta)
                        {
                            var current = selected.IndexOf(modelId); var target = current + delta;
                            if (target < 0 || target >= selected.Count) return;
                            selected.RemoveAt(current); selected.Insert(target, modelId); draftDirty = true; Rows();
                        }
                        var up = new Arc.Button { Content = Localized.Get("上移"), Variant = Arc.ButtonVariant.Ghost, Size = Arc.ControlSize.Small };
                        var down = new Arc.Button { Content = Localized.Get("下移"), Variant = Arc.ButtonVariant.Ghost, Size = Arc.ControlSize.Small };
                        up.Click += (_, _) => Move(-1); down.Click += (_, _) => Move(1);
                        tools.Children.Add(up); tools.Children.Add(down);
                    }
                    Grid.SetColumn(tools, 1); row.Children.Add(tools);
                }
                list.Children.Add(row);
            }
            if (list.Children.Count == 0)
                list.Children.Add(new Arc.EmptyState { Size = Arc.ControlSize.Small, Title = Localized.Get("没有可显示的模型"), Description = Localized.Get("刷新模型列表，或调整搜索与筛选。") });
        }
        search.TextChanged += (_, _) => Rows();
        filter.SelectionChanged += (_, _) => Rows();
        orderSelect.SelectionChanged += (_, _) => { draftDirty = true; Rows(); };

        var actions = new WrapPanel { Orientation = Orientation.Horizontal };
        Arc.Button Action(string text, Arc.ButtonVariant variant, Func<Task> run)
        {
            var button = new Arc.Button { Content = text, Variant = variant, Margin = new Thickness(0, 0, 8, 8) };
            button.Click += async (_, _) => await run();
            return button;
        }
        var apply = Action(Localized.Get("保存模型选择"), Arc.ButtonVariant.Primary, async () =>
        {
            var args = new List<string> { "provider", "select", id, "--model-order", ProviderValue(orderSelect) };
            foreach (var modelId in selected) args.AddRange(["--model", modelId]);
            foreach (var (modelId, value) in contexts)
            {
                if (double.IsNaN(value)) { if (savedContexts.TryGetProperty(modelId, out _)) args.AddRange(["--clear-model-context", modelId]); continue; }
                if (!double.IsFinite(value) || value <= 0 || value != Math.Truncate(value))
                {
                    error.Text = Localized.Get("上下文 Token 数必须是正整数。"); error.IsVisible = true; return;
                }
                args.AddRange(["--model-context", $"{modelId}={value:0}"]);
            }
            error.IsVisible = false;
            await RunActionAsync(args.ToArray(), apply: true);
        });
        test.Click += async (_, _) => await RunActionAsync(["benchmark", "start", "--provider", id, "--timeout-seconds", "60"]);
        test.Margin = new Thickness(0, 0, 8, 8);
        AutomationProperties.SetAutomationId(apply, "provider-models-save");
        AutomationProperties.SetAutomationId(test, "provider-models-benchmark");
        actions.Children.Add(apply);
        var discover = Action(Localized.Get("刷新模型列表"), Arc.ButtonVariant.Secondary, () => RunActionAsync(["provider", "discover", id], apply: true));
        var probe = Action(Localized.Get("重新探测已加入模型的能力"), Arc.ButtonVariant.Secondary, () => RunActionAsync(["provider", "probe", id], apply: true));
        AutomationProperties.SetAutomationId(discover, "provider-models-discover");
        AutomationProperties.SetAutomationId(probe, "provider-models-probe");
        actions.Children.Add(discover);
        actions.Children.Add(probe);
        actions.Children.Add(test);

        foreach (var control in new Control[] { search, filter, orderSelect, scroll, error, actions }) panel.Children.Add(control);
        Count(); Rows();
        return panel;
    }
}
