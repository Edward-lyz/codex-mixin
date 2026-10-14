using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Windows.Automation;

namespace CodexMixin.UiTests;

internal static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        if (args.Length != 2) throw new ArgumentException("Usage: UiTests <UI exe> <artifact directory>");
        var output = Path.GetFullPath(args[1]);
        Directory.CreateDirectory(output);
        var config = Path.Combine(output, "isolated", "config.json");
        Directory.CreateDirectory(Path.GetDirectoryName(config)!);
        var start = new ProcessStartInfo(Path.GetFullPath(args[0])) { UseShellExecute = false };
        start.Environment["CODEX_GATEWAY_CONFIG"] = config;
        var previousConfig = Environment.GetEnvironmentVariable("CODEX_GATEWAY_CONFIG");
        Environment.SetEnvironmentVariable("CODEX_GATEWAY_CONFIG", config);
        using var process = Process.Start(start) ?? throw new InvalidOperationException("Cannot launch WinUI app");
        nint handle = 0;
        try
        {
            Wait(() => { process.Refresh(); handle = process.MainWindowHandle; return handle != 0; }, "native main window", process);
            var desktop = System.Windows.Forms.Screen.FromHandle(handle).WorkingArea;
            if (desktop.Width < 800 || desktop.Height < 600)
                throw new InvalidOperationException("CI desktop is too small to capture the native application");
            MoveWindow(handle, desktop.Left + 24, desktop.Top + 24,
                Math.Min(1120, desktop.Width - 48), Math.Min(800, desktop.Height - 48), true);
            var window = AutomationElement.FromHandle(handle);
            Wait(() => Find(window, "operation-status").Current.Name.Contains("完成", StringComparison.Ordinal), "initial CLI load", process);
            AssertName(window, "本地网关 · 未配置");
            Console.WriteLine("PASS fresh installation shows unconfigured gateway");
            Capture(window, Path.Combine(output, "overview-unconfigured.png"));
            Navigate(window, "nav-providers", "管理服务连接与向 Agent 提供的模型", process);
            AssertName(window, "暂无供应商");
            Console.WriteLine("PASS provider empty state");
            Capture(window, Path.Combine(output, "providers-empty.png"));
            Navigate(window, "nav-agents", "检测、接入与升级由本地 CLI 执行", process);
            AssertName(window, "检查版本更新");
            Console.WriteLine("PASS agent contract reaches native controls");
            Capture(window, Path.Combine(output, "agents-detected.png"));
            Navigate(window, "nav-usage", "真实请求用量、缓存命中与供应商额度", process);
            AssertName(window, "用量不可用");
            Console.WriteLine("PASS stopped gateway does not show zero usage");
            Capture(window, Path.Combine(output, "usage-stopped.png"));
            Navigate(window, "nav-fusion", "组合 Panel、Judge 与 Final 模型", process);
            AssertName(window, "暂无可用模型");
            Console.WriteLine("PASS Fusion empty state");
            Capture(window, Path.Combine(output, "fusion-empty.png"));
            Navigate(window, "nav-settings", "外观、启动、备份与诊断", process);
            SeedDemo(Path.Combine(Path.GetDirectoryName(start.FileName)!, "codex-mixin.exe"), output);
            foreach (var theme in new[] { "light", "dark" })
            {
                SelectTheme(window, theme == "light" ? "浅色" : "深色");
                foreach (var (id, description, filename) in Pages)
                {
                    Navigate(window, id, description, process);
                    Capture(window, Path.Combine(output, $"{filename}-{theme}.png"));
                    var viewport = Find(window, "page-scroll");
                    if (viewport.TryGetCurrentPattern(ScrollPattern.Pattern, out var pattern) &&
                        pattern is ScrollPattern { Current.VerticallyScrollable: true } scroll)
                    {
                        scroll.SetScrollPercent(ScrollPattern.NoScroll, 100);
                        Capture(window, Path.Combine(output, $"{filename}-{theme}-lower.png"));
                        scroll.SetScrollPercent(ScrollPattern.NoScroll, 0);
                    }
                }
            }
            Console.WriteLine("PASS all six native pages captured in light and dark themes");
            // WM_SYSCOMMAND/SC_CLOSE must hide the shell without terminating it.
            PostMessage(handle, 0x112, 0xF060, 0);
            Wait(() => !IsWindowVisible(handle), "close to tray", process);
            using var second = Process.Start(start) ?? throw new InvalidOperationException("Cannot reactivate app");
            Wait(() => IsWindowVisible(handle), "single-instance reactivation", process);
            if (!second.WaitForExit(15000)) throw new InvalidOperationException("Second instance did not redirect activation");
            if (second.ExitCode != 0) throw new InvalidOperationException("Second instance did not exit cleanly");
            Console.WriteLine("PASS close-to-tray and single-instance activation");
            MoveWindow(handle, 100, 100, 760, 560, true);
            Thread.Sleep(500);
            if (Find(window, "nav-settings").Current.IsOffscreen)
                throw new InvalidOperationException("Navigation is inaccessible after resize");
            Capture(window, Path.Combine(output, "minimum-window.png"));
            Console.WriteLine("PASS native window remains accessible after resize");
            return 0;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine(error);
            File.WriteAllText(Path.Combine(output, "failure.txt"), error.ToString());
            if (handle != 0 && !process.HasExited)
            {
                try { Capture(AutomationElement.FromHandle(handle), Path.Combine(output, "failure.png")); }
                catch (Exception captureError) { Console.Error.WriteLine($"Failure screenshot unavailable: {captureError.Message}"); }
            }
            return 1;
        }
        finally
        {
            if (!process.HasExited) { process.Kill(true); process.WaitForExit(); }
            Environment.SetEnvironmentVariable("CODEX_GATEWAY_CONFIG", previousConfig);
            WriteGallery(output);
        }
    }

    private static readonly (string Id, string Description, string Filename)[] Pages =
    [
        ("nav-overview", "模型服务、连接状态与常用操作", "overview"),
        ("nav-providers", "管理服务连接与向 Agent 提供的模型", "providers"),
        ("nav-agents", "检测、接入与升级由本地 CLI 执行", "agents"),
        ("nav-usage", "真实请求用量、缓存命中与供应商额度", "usage"),
        ("nav-fusion", "组合 Panel、Judge 与 Final 模型", "fusion"),
        ("nav-settings", "外观、启动、备份与诊断", "settings")
    ];

    private static void SelectTheme(AutomationElement window, string name)
    {
        var theme = Find(window, "theme-picker");
        ((ExpandCollapsePattern)theme.GetCurrentPattern(ExpandCollapsePattern.Pattern)).Expand();
        var option = window.FindFirst(TreeScope.Descendants, new AndCondition(
            new PropertyCondition(AutomationElement.NameProperty, name),
            new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.ListItem)))
            ?? throw new InvalidOperationException($"Theme option is missing: {name}");
        ((SelectionItemPattern)option.GetCurrentPattern(SelectionItemPattern.Pattern)).Select();
        Thread.Sleep(400);
    }

    private static void SeedDemo(string executable, string output)
    {
        // This fixture is imported by the real CLI. No provider or gateway result is mocked.
        // It uses reserved .invalid hosts and fictional credentials, with no network actions.
        var models = new[] { "ci-coder", "ci-reasoner", "ci-vision" };
        var document = new
        {
            config_version = 2,
            providers = new[] { new
            {
                id = "ci-demo", display_name = "CI 示例供应商", preset_id = "custom", enabled = true,
                protocol = "open_ai_chat", base_url = "https://example.invalid", api_path = "/v1/chat/completions",
                model_source = new { kind = "static" },
                auth = new { header = "authorization_bearer", api_key = "ci-fixture-not-a-real-key" },
                selected_models = models,
                cached_models = models.Select(id => new { id, display_name = id, context_window = 128000, max_output_tokens = 8192 }),
                model_context_overrides = new Dictionary<string, int> { ["ci-reasoner"] = 64000 },
                models_refreshed_at_ms = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()
            } }
        };
        var backup = Path.Combine(output, "isolated", "demo.b64");
        File.WriteAllText(backup, Convert.ToBase64String(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(document))));
        var cli = new CodexMixin.CliClient(executable);
        cli.RunAsync(["config", "import", backup], CancellationToken.None).GetAwaiter().GetResult();
        var options = CodexMixin.Json.Rows(CodexMixin.Json.Field(cli.QueryAsync(["fusion", "models", "--json"], CancellationToken.None).GetAwaiter().GetResult(), "models"));
        var ids = options.Select(row => CodexMixin.Json.Text(row, "id")).ToArray();
        if (ids.Length < 2) throw new InvalidOperationException("Demo configuration did not expose model options");
        var profile = JsonSerializer.Serialize(new
        {
            id = "ci-demo", panel_models = ids.Take(3).ToArray(), judge_model = ids[0], final_model = ids[1],
            min_successful = 1, timeout_ms = 300000, max_completion_tokens = 2048,
            show_intermediate_results = true,
            panel_tools = new { enabled = true, max_rounds = 16, max_calls_per_model = 64 }
        });
        cli.RunAsync(["fusion", "set", "--profile-json", profile], CancellationToken.None).GetAwaiter().GetResult();
    }

    private static void WriteGallery(string output)
    {
        var images = Directory.GetFiles(output, "*.png").OrderBy(Path.GetFileName).ToArray();
        var figures = string.Join("\n", images.Select(path =>
        {
            var filename = Uri.EscapeDataString(Path.GetFileName(path));
            var caption = System.Net.WebUtility.HtmlEncode(Path.GetFileNameWithoutExtension(path));
            return $"<figure><a href='{filename}'><img src='{filename}' alt='{caption}' loading='lazy'></a><figcaption>{caption}</figcaption></figure>";
        }));
        var html = "<!doctype html><html lang='zh-CN'><meta charset='utf-8'><meta name='viewport' content='width=device-width,initial-scale=1'>" +
            "<title>Codex Mixin Windows UI · GitHub CI</title><style>body{margin:32px;font:15px system-ui;background:#101318;color:#e8edf5}h1{font-size:28px}p{color:#aeb8c8}main{display:grid;grid-template-columns:repeat(auto-fit,minmax(400px,1fr));gap:24px}figure{margin:0;padding:12px;background:#1c222b;border-radius:12px}img{width:100%;height:auto;border-radius:6px}figcaption{padding-top:12px}a{color:inherit}@media(max-width:480px){body{margin:12px}main{grid-template-columns:1fr}}</style>" +
            "<h1>Codex Mixin · Windows 原生界面</h1><p>由 GitHub Windows runner 运行真实 WinUI 3 应用并截图。示例供应商和 Fusion 来自 CLI 导入的隔离 CI fixture，凭据为虚构值；未连接外部模型服务。</p><main>" + figures + "</main></html>";
        File.WriteAllText(Path.Combine(output, "index.html"), html);
    }

    private static AutomationElement Find(AutomationElement root, string id) =>
        root.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.AutomationIdProperty, id))
        ?? throw new InvalidOperationException($"Control is missing: {id}");

    private static void Navigate(AutomationElement window, string id, string description, Process process)
    {
        var control = Find(window, id);
        if (control.TryGetCurrentPattern(SelectionItemPattern.Pattern, out var selection))
            ((SelectionItemPattern)selection).Select();
        else ((InvokePattern)control.GetCurrentPattern(InvokePattern.Pattern)).Invoke();
        Wait(() => Find(window, "page-description").Current.Name == description &&
            Find(window, "operation-status").Current.Name.Contains("完成", StringComparison.Ordinal), $"navigate {id}", process);
    }

    private static void AssertName(AutomationElement window, string name)
    {
        if (window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, name)) == null)
            throw new InvalidOperationException($"Expected visible content: {name}");
    }

    private static void Wait(Func<bool> condition, string label, Process process)
    {
        var deadline = DateTime.UtcNow.AddSeconds(90);
        while (DateTime.UtcNow < deadline)
        {
            if (process.HasExited) throw new InvalidOperationException($"WinUI exited during {label}, code {process.ExitCode}");
            try { if (condition()) return; }
            catch (ElementNotAvailableException) { }
            catch (InvalidOperationException) { /* XAML automation peers may appear after HWND creation. */ }
            Thread.Sleep(200);
        }
        throw new TimeoutException($"Timed out: {label}");
    }

    private static void Capture(AutomationElement window, string path)
    {
        Thread.Sleep(300);
        var result = DwmGetWindowAttribute((nint)window.Current.NativeWindowHandle, 9, out var rect, Marshal.SizeOf<FrameRect>());
        if (result != 0) throw new InvalidOperationException($"Cannot read the visible window frame: HRESULT {result:X8}");
        using var image = new Bitmap(rect.Right - rect.Left, rect.Bottom - rect.Top);
        using var graphics = Graphics.FromImage(image);
        graphics.CopyFromScreen(rect.Left, rect.Top, 0, 0, image.Size);
        image.Save(path, ImageFormat.Png);
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct FrameRect { internal int Left; internal int Top; internal int Right; internal int Bottom; }
    [DllImport("dwmapi.dll")]
    private static extern int DwmGetWindowAttribute(nint window, uint attribute, out FrameRect value, int size);

    [DllImport("user32.dll", EntryPoint = "PostMessageW")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool PostMessage(nint window, uint message, nuint wParam, nint lParam);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool IsWindowVisible(nint window);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool MoveWindow(nint window, int x, int y, int width, int height, bool repaint);
}
