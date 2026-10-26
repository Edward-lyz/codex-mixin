using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Net.Http.Json;
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
        if (args.Length is not (2 or 3)) throw new ArgumentException("Usage: UiTests <UI exe> <artifact directory> [native debugger]");
        using (var icon = new Bitmap(Path.Combine(Path.GetDirectoryName(Path.GetFullPath(args[0]))!, "Assets", "app_icon.png")))
            if (icon.GetPixel(0, 0).A != 0) throw new InvalidOperationException("Windows application icon still has an opaque square background");
        var output = Path.GetFullPath(args[1]);
        Directory.CreateDirectory(output);
        var config = Path.Combine(output, "isolated", "config.json");
        Directory.CreateDirectory(Path.GetDirectoryName(config)!);
        var start = new ProcessStartInfo(Path.GetFullPath(args[0])) { UseShellExecute = false, RedirectStandardError = true };
        start.Environment["CODEX_GATEWAY_CONFIG"] = config;
        start.ArgumentList.Add("--capture-ui");
        start.ArgumentList.Add("--background");
        var previousConfig = Environment.GetEnvironmentVariable("CODEX_GATEWAY_CONFIG");
        Environment.SetEnvironmentVariable("CODEX_GATEWAY_CONFIG", config);
        Process? debugger = null;
        Process process;
        Task<string> nativeErrors;
        if (args.Length == 3)
        {
            var commands = Path.Combine(output, "native-crash.commands");
            File.WriteAllText(commands, "sxe -c \".exr -1; !analyze -v; q\" 0xc000027b\ng\n");
            var debugStart = new ProcessStartInfo(args[2]) { UseShellExecute = false };
            debugStart.Environment["CODEX_GATEWAY_CONFIG"] = config;
            foreach (var argument in new[] { "-G", "-cf", commands, "-logo", Path.Combine(output, "native-debugger.txt"), start.FileName, "--capture-ui", "--background" })
                debugStart.ArgumentList.Add(argument);
            debugger = Process.Start(debugStart) ?? throw new InvalidOperationException("Cannot launch native debugger");
            Process? target = null;
            var deadline = DateTime.UtcNow.AddSeconds(15);
            while (target == null && DateTime.UtcNow < deadline)
            {
                target = Process.GetProcessesByName(Path.GetFileNameWithoutExtension(start.FileName)).SingleOrDefault();
                if (target == null) Thread.Sleep(100);
            }
            process = target ?? throw new InvalidOperationException("Native debugger did not launch WinUI");
            nativeErrors = Task.FromResult("");
        }
        else
        {
            process = Process.Start(start) ?? throw new InvalidOperationException("Cannot launch WinUI app");
            nativeErrors = process.StandardError.ReadToEndAsync();
        }
        using var appLifetime = process;
        using var upstream = new FixtureHost();
        var cli = new CodexMixin.CliClient(Path.Combine(Path.GetDirectoryName(start.FileName)!, "codex-mixin.exe"));
        var gatewayStarted = false;
        _ = process.Handle; // Retain the debugger-launched process handle for exit diagnostics.
        nint handle = 0;
        try
        {
            using var ready = new EventWaitHandle(false, EventResetMode.ManualReset, $"Local\\CodexMixin.UIReady.{process.Id}");
            using var trayReady = new EventWaitHandle(false, EventResetMode.ManualReset, $"Local\\CodexMixin.TrayReady.{process.Id}");
            Wait(() => trayReady.WaitOne(0), "tray instance ready to receive activation", process);
            if (!process.WaitForInputIdle(15000)) throw new InvalidOperationException("Background app did not initialize its message loop");
            var visibleAtStartup = false;
            if (!EnumWindows((candidate, _) =>
            {
                GetWindowThreadProcessId(candidate, out var owner);
                if (owner == process.Id && IsWindowVisible(candidate)) visibleAtStartup = true;
                return true;
            }, 0)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
            if (visibleAtStartup || ready.WaitOne(0)) throw new InvalidOperationException("Background startup displayed or loaded the console");
            process.Refresh();
            var backgroundPrivateBytes = process.PrivateMemorySize64;
            var revealStart = new ProcessStartInfo(start.FileName) { UseShellExecute = false };
            foreach (var variable in start.Environment) revealStart.Environment[variable.Key] = variable.Value;
            revealStart.ArgumentList.Add("--page=providers");
            using var reveal = Process.Start(revealStart) ?? throw new InvalidOperationException("Cannot reveal background console");
            Wait(() => ready.WaitOne(0), "completed native initialization", process);
            if (!reveal.WaitForExit(15000) || reveal.ExitCode != 0) throw new InvalidOperationException("Background activation failed");
            Console.WriteLine("PASS tray-only startup defers console data and rendering");
            Wait(() => { process.Refresh(); handle = process.MainWindowHandle; return handle != 0; }, "native main window", process);
            var desktop = System.Windows.Forms.Screen.FromHandle(handle).WorkingArea;
            if (desktop.Width < 800 || desktop.Height < 600)
                throw new InvalidOperationException("CI desktop is too small to capture the native application");
            AutomationElement? loadedWindow = null;
            Wait(() =>
            {
                loadedWindow = AutomationElement.RootElement.FindFirst(TreeScope.Children, new AndCondition(
                    new PropertyCondition(AutomationElement.ProcessIdProperty, process.Id),
                    new PropertyCondition(AutomationElement.NameProperty, "模型与服务")));
                if (loadedWindow == null) return false;
                handle = (nint)loadedWindow.Current.NativeWindowHandle;
                return !loadedWindow.Current.IsOffscreen;
            }, "native automation window", process);
            var window = loadedWindow!;
            // MainWindowHandle can initially refer to a WinUI helper HWND.
            // Resize only the application window identified by its title and PID.
            MoveWindow(handle, desktop.Left + 24, desktop.Top + 24,
                Math.Min(1120, desktop.Width - 48), Math.Min(800, desktop.Height - 48), true);
            Wait(() => Find(window, "operation-status").Current.Name.Contains("完成", StringComparison.Ordinal), "initial CLI load", process);
            AssertName(window, "还没有服务商");
            Capture(window, Path.Combine(output, "providers-empty.png"));
            var modelIds = SeedDemo(cli, output, upstream.Url);
            var gateway = $"http://127.0.0.1:{FixtureHost.FreePort()}";
            cli.RunAsync(["service", "start", "--bind", new Uri(gateway).Authority, "--json"], CancellationToken.None, timeout: TimeSpan.FromSeconds(30)).GetAwaiter().GetResult();
            gatewayStarted = true;
            SeedTraffic(gateway, modelIds).GetAwaiter().GetResult();
            window = OpenWindow(start, process, "providers", "模型与服务");
            var refreshProviders = window.FindFirst(TreeScope.Descendants, new AndCondition(
                new PropertyCondition(AutomationElement.NameProperty, "刷新"),
                new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Button)))!;
            ((InvokePattern)refreshProviders.GetCurrentPattern(InvokePattern.Pattern)).Invoke();
            Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "ci-coder")) != null, "seeded provider editor", process);
            ((TogglePattern)Find(window, "provider-connection-tab").GetCurrentPattern(TogglePattern.Pattern)).Toggle();
            Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.AutomationIdProperty, "provider-base-url")) != null, "connection draft editor", process);
            var originalUrl = ((ValuePattern)Find(window, "provider-base-url").GetCurrentPattern(ValuePattern.Pattern)).Current.Value;
            ((ValuePattern)Find(window, "provider-base-url").GetCurrentPattern(ValuePattern.Pattern)).SetValue("not-a-url");
            ((TogglePattern)Find(window, "provider-models-tab").GetCurrentPattern(TogglePattern.Pattern)).Toggle();
            Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "放弃更改")) != null, "connection draft confirmation", process);
            ((InvokePattern)Find(window, "CloseButton").GetCurrentPattern(InvokePattern.Pattern)).Invoke();
            Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "放弃更改")) == null, "cancelled draft navigation", process);
            Assert(((ValuePattern)Find(window, "provider-base-url").GetCurrentPattern(ValuePattern.Pattern)).Current.Value == "not-a-url", "Cancelled navigation lost the connection draft");
            ((InvokePattern)Find(window, "apply-changes").GetCurrentPattern(InvokePattern.Pattern)).Invoke();
            Wait(() => Find(window, "operation-status").Current.Name.Contains("失败", StringComparison.Ordinal), "invalid URL save failure", process);
            Assert(((ValuePattern)Find(window, "provider-base-url").GetCurrentPattern(ValuePattern.Pattern)).Current.Value == "not-a-url", "Failed save cleared the connection draft");
            Assert(Find(window, "apply-changes").Current.HelpText.Contains("未应用", StringComparison.Ordinal), "Failed save reported success instead of retaining a dirty draft");
            ((TogglePattern)Find(window, "provider-models-tab").GetCurrentPattern(TogglePattern.Pattern)).Toggle();
            Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "放弃更改")) != null, "discard connection draft", process);
            ((InvokePattern)Find(window, "PrimaryButton").GetCurrentPattern(InvokePattern.Pattern)).Invoke();
            Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.AutomationIdProperty, "provider-base-url")) == null, "model tab after discard", process);
            ((TogglePattern)Find(window, "provider-connection-tab").GetCurrentPattern(TogglePattern.Pattern)).Toggle();
            Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.AutomationIdProperty, "provider-base-url")) != null, "saved connection after discard", process);
            Assert(((ValuePattern)Find(window, "provider-base-url").GetCurrentPattern(ValuePattern.Pattern)).Current.Value == originalUrl, "Discard did not reload the saved connection");
            ((TogglePattern)Find(window, "provider-models-tab").GetCurrentPattern(TogglePattern.Pattern)).Toggle();
            for (var themeIndex = 0; themeIndex < 2; themeIndex++)
            {
                var themeName = themeIndex == 0 ? "light" : "dark";
                var settings = OpenWindow(start, process, "settings", "设置与诊断");
                Assert(Find(settings, "open-gui-logs").Current.IsEnabled && Find(settings, "check-app-updates").Current.IsEnabled,
                    "Recovery and update controls are not available independently of CLI settings queries");
                SelectTheme(settings, themeIndex == 0 ? "浅色" : "深色");
                foreach (var (role, title, filename) in Pages)
                {
                    var target = OpenWindow(start, process, role, title);
                    if (role != "overview")
                    {
                        MoveWindow((nint)target.Current.NativeWindowHandle, desktop.Left + 24, desktop.Top + 24,
                            Math.Min(1120, desktop.Width - 48), Math.Min(800, desktop.Height - 48), true);
                        var refresh = target.FindFirst(TreeScope.Descendants, new AndCondition(new PropertyCondition(AutomationElement.NameProperty, "刷新"), new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Button)));
                        if (refresh != null) ((InvokePattern)refresh.GetCurrentPattern(InvokePattern.Pattern)).Invoke();
                        Wait(() => Find(target, "operation-status").Current.Name.Contains("完成", StringComparison.Ordinal), $"load {role}", process);
                    }
                    if (role == "providers")
                        Wait(() =>
                        {
                            var model = target.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "ci-coder"));
                            return model != null && !model.Current.IsOffscreen && model.Current.BoundingRectangle.Height > 0;
                        }, "painted model table rows", process);
                    if (role == "overview")
                    {
                        Wait(() => Find(target, "panel-status").Current.Name.Contains("配置已同步", StringComparison.Ordinal), "tray CLI load", process);
                        Wait(() => target.FindAll(TreeScope.Descendants, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Text)).Cast<AutomationElement>()
                            .Any(element => element.Current.Name.Contains("81 / 100", StringComparison.Ordinal)), "real CLI quota display", process);
                        AssertQuotaTextContrast(target, themeIndex == 0);
                    }
                    if (role == "usage")
                        Wait(() => target.FindAll(TreeScope.Descendants, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Text)).Cast<AutomationElement>()
                            .Any(element => element.Current.Name.Contains("ci-coder", StringComparison.Ordinal)), "real gateway per-turn usage", process);
                    if (role == "context")
                        Wait(() => Find(target, "context-enabled") != null && Find(target, "context-save") != null,
                            "context settings controls", process);
                    if (role == "settings" && target.FindFirst(TreeScope.Descendants,
                        new PropertyCondition(AutomationElement.NameProperty, "打开上下文设置")) != null)
                        throw new InvalidOperationException("Settings still has the redundant context entry");
                    Capture(target, Path.Combine(output, $"{filename}-{themeName}.png"));
                    if (role != "overview") CaptureLower(target, Path.Combine(output, $"{filename}-lower-{themeName}.png"));
                    if (role == "agents")
                    {
                        var folded = target.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.AutomationIdProperty, "agent-undetected"));
                        if (folded != null)
                        {
                            var expand = (ExpandCollapsePattern)folded.GetCurrentPattern(ExpandCollapsePattern.Pattern);
                            expand.Expand();
                            Capture(target, Path.Combine(output, $"agents-undetected-{themeName}.png"));
                            CaptureLower(target, Path.Combine(output, $"agents-undetected-lower-{themeName}.png"));
                            expand.Collapse();
                        }
                    }
                    if (role == "providers")
                    {
                        ((TogglePattern)Find(target, "provider-connection-tab").GetCurrentPattern(TogglePattern.Pattern)).Toggle();
                        Wait(() => target.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "连接配置")) != null, "connection tab", process);
                        Capture(target, Path.Combine(output, $"connection-{themeName}.png"));
                        CaptureLower(target, Path.Combine(output, $"connection-lower-{themeName}.png"));
                        ((TogglePattern)Find(target, "provider-models-tab").GetCurrentPattern(TogglePattern.Pattern)).Toggle();
                    }
                    if (role == "fusion")
                    {
                        SelectCombo(target, "fusion-mode", "按时间轮转");
                        Capture(target, Path.Combine(output, $"fusion-time-{themeName}.png"));
                        CaptureLower(target, Path.Combine(output, $"fusion-time-lower-{themeName}.png"));
                        SelectCombo(target, "fusion-mode", "多模型编排");
                        var fusionHandle = (nint)target.Current.NativeWindowHandle;
                        ((WindowPattern)target.GetCurrentPattern(WindowPattern.Pattern)).Close();
                        Wait(() => target.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "放弃更改")) != null, "Fusion close protects its draft", process);
                        ((InvokePattern)Find(target, "PrimaryButton").GetCurrentPattern(InvokePattern.Pattern)).Invoke();
                        Wait(() => !IsWindowVisible(fusionHandle), "Fusion closes after explicit discard", process);
                    }
                }
            }
            window = OpenWindow(start, process, "providers", "模型与服务");
            var trayOwner = (nint)window.Current.NativeWindowHandle;
            var releaseQuota = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
            upstream.QuotaRelease = releaseQuota;
            try
            {
                ((TogglePattern)Find(window, "console-usage-tab").GetCurrentPattern(TogglePattern.Pattern)).Toggle();
                Wait(() => window.Current.Name == "用量明细" && Find(window, "operation-status").Current.Name.Contains("完成", StringComparison.Ordinal), "usage loads without a supplier quota dependency", process);
                if (window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "供应商额度")) != null)
                    throw new InvalidOperationException("Usage page still contains supplier quotas");
                // Exercise the real tray notification path while remote quota is held.
                PostMessage(trayOwner, 0x8001, 0, 0x202);
                AutomationElement? popup = null;
                Wait(() =>
                {
                    popup = AutomationElement.RootElement.FindFirst(TreeScope.Children, new AndCondition(
                        new PropertyCondition(AutomationElement.ProcessIdProperty, process.Id),
                        new PropertyCondition(AutomationElement.NameProperty, "Codex Mixin · 主面板")));
                    return popup != null && !popup.Current.IsOffscreen;
                }, "tray opens while console query is pending", process);
                var popupHandle = (nint)popup!.Current.NativeWindowHandle;
                if (string.IsNullOrWhiteSpace(Find(popup, "quota-stamp").Current.Name))
                    throw new InvalidOperationException("Reopened tray discarded its last measured quota and timestamp");
                ((InvokePattern)Find(popup, "tray-refresh").GetCurrentPattern(InvokePattern.Pattern)).Invoke();
                Wait(() => upstream.QuotaRequested.Task.IsCompleted, "explicit tray quota request held in flight", process);
                Wait(() => Find(popup, "panel-status").Current.Name.Contains("正在刷新", StringComparison.Ordinal), "explicit tray refresh bypasses its recent snapshot", process);
                Wait(() => popup.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "本地网关"))
                    is { Current.IsEnabled: true }, "gateway control becomes usable before network quota completes", process);
                var popupBounds = popup.Current.BoundingRectangle;
                if (popupBounds.Height > popupBounds.Width * 1.8)
                    throw new InvalidOperationException("Tray kept its maximum-height blank space while quota was pending");
                if (Find(popup, "panel-status").Current.Name.Contains("配置已同步", StringComparison.Ordinal))
                    throw new InvalidOperationException("Tray claimed quota sync before the held request completed");
                Capture(popup, Path.Combine(output, "tray-slow-quota.png"));
                // Hosted runners cannot grant foreground focus to another HWND.
                // Deliver Windows activation messages through the real WinUI window proc.
                PostMessage(popupHandle, 0x6, 1, 0); // WM_ACTIVATE / WA_ACTIVE
                PostMessage(popupHandle, 0x6, 0, trayOwner); // WM_ACTIVATE / WA_INACTIVE
                Wait(() => !IsWindowVisible(popupHandle), "tray dismisses on deactivation during reload", process);
                PostMessage(trayOwner, 0x8001, 0, 0x202);
                Wait(() => IsWindowVisible(popupHandle), "tray reopens during slow quota", process);
                ((InvokePattern)Find(popup, "tray-refresh").GetCurrentPattern(InvokePattern.Pattern)).Invoke();
                Wait(() => Find(popup, "panel-status").Current.Name.Contains("正在刷新", StringComparison.Ordinal), "reopened tray starts a new read", process);
                Wait(() => popup.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "本地网关"))
                    is { Current.IsEnabled: true }, "reopened tray keeps local controls responsive", process);
                PostMessage(popupHandle, 0x6, 1, 0);
                PostMessage(popupHandle, 0x6, 0, trayOwner);
                Wait(() => !IsWindowVisible(popupHandle), "reopened tray dismisses", process);
            }
            finally
            {
                releaseQuota.TrySetResult();
                upstream.QuotaRelease = null;
            }
            Console.WriteLine("PASS tray dismisses while loading");
            window = OpenWindow(start, process, "providers", "模型与服务");
            var consoleHandle = (nint)window.Current.NativeWindowHandle;
            var pattern = (WindowPattern)window.GetCurrentPattern(WindowPattern.Pattern);
            pattern.SetWindowVisualState(WindowVisualState.Maximized);
            Wait(() => IsZoomed(consoleHandle), "maximized console", process);
            var navigationMs = new Dictionary<string, double>();
            for (var navigationPass = 1; navigationPass <= 2; navigationPass++)
            {
                foreach (var (role, title) in new[] { ("agents", "Agent 接入管理"), ("usage", "用量明细"), ("context", "上下文"), ("providers", "模型与服务") })
                {
                    var elapsed = Stopwatch.StartNew();
                    Wait(() => Find(window, $"console-{role}-tab").TryGetCurrentPattern(TogglePattern.Pattern, out _), $"ready console tab {role}", process);
                    ((TogglePattern)Find(window, $"console-{role}-tab").GetCurrentPattern(TogglePattern.Pattern)).Toggle();
                    Wait(() => window.Current.Name == title && Find(window, "operation-status").Current.Name.Contains("完成", StringComparison.Ordinal), $"console tab {role}", process);
                    navigationMs[$"{role}-{navigationPass}"] = elapsed.Elapsed.TotalMilliseconds;
                    if ((nint)window.Current.NativeWindowHandle != consoleHandle || !IsZoomed(consoleHandle))
                        throw new InvalidOperationException($"Console lost maximization while opening {role}");
                }
            }
            // Reopening through the tray/second-instance route must also preserve maximization.
            window = OpenWindow(start, process, "providers", "模型与服务");
            if (!IsZoomed(consoleHandle)) throw new InvalidOperationException("Reactivation lost maximization");
            pattern.SetWindowVisualState(WindowVisualState.Normal);
            Console.WriteLine("PASS console navigation preserves maximization");
            SeedTraffic(gateway, modelIds, repetitions: 249).GetAwaiter().GetResult();
            var recentTurns = CodexMixin.Json.Rows(cli.QueryAsync(["usage", "--turns", "--json", "--days", "7", "--limit", "500"], CancellationToken.None).GetAwaiter().GetResult());
            if (recentTurns.Length != 500) throw new InvalidOperationException($"Expected 500 real gateway turns, got {recentTurns.Length}");
            var usageScale = OpenWindow(start, process, "usage", "用量明细");
            Wait(() => usageScale.FindAll(TreeScope.Descendants, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Text))
                .Cast<AutomationElement>().Any(element => element.Current.Name.Contains("ci-coder", StringComparison.Ordinal)), "large reply ledger has painted rows", process);
            var usageTextNodes = usageScale.FindAll(TreeScope.Descendants, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Text)).Count;
            if (usageTextNodes >= 1000) throw new InvalidOperationException($"Reply ledger eagerly created {usageTextNodes} text controls for 500 turns");
            Capture(usageScale, Path.Combine(output, "usage-500-turns.png"));
            SeedDemo(cli, output, upstream.Url, modelCount: 600);
            var fusionScale = OpenWindow(start, process, "fusion", "Fusion 设置");
            var fusionRefresh = fusionScale.FindFirst(TreeScope.Descendants, new AndCondition(
                new PropertyCondition(AutomationElement.NameProperty, "刷新"), new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Button)))
                ?? throw new InvalidOperationException("Fusion refresh button missing");
            ((InvokePattern)fusionRefresh.GetCurrentPattern(InvokePattern.Pattern)).Invoke();
            Wait(() => fusionScale.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "可选 600 个模型")) != null
                && Find(fusionScale, "operation-status").Current.Name.Contains("完成", StringComparison.Ordinal), "large Fusion catalog load", process);
            var fusionChecks = fusionScale.FindAll(TreeScope.Descendants, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.CheckBox)).Count;
            if (fusionChecks >= 100) throw new InvalidOperationException($"Fusion eagerly created {fusionChecks} checkbox controls for 600 models");
            Capture(fusionScale, Path.Combine(output, "fusion-600-models.png"));
            var modelsScale = OpenWindow(start, process, "providers", "模型与服务");
            // The fixture changed config outside the GUI; explicitly invalidate its recent snapshot.
            var modelRefresh = modelsScale.FindFirst(TreeScope.Descendants, new AndCondition(
                new PropertyCondition(AutomationElement.NameProperty, "刷新"), new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Button)))
                ?? throw new InvalidOperationException("Model refresh button missing");
            ((InvokePattern)modelRefresh.GetCurrentPattern(InvokePattern.Pattern)).Invoke();
            Wait(() => modelsScale.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "测试已加入的 600 个模型")) != null
                && Find(modelsScale, "operation-status").Current.Name.Contains("完成", StringComparison.Ordinal), "manual refresh observes external catalog changes", process);
            var table = Find(modelsScale, "model-table");
            var scroll = (ScrollPattern)table.GetCurrentPattern(ScrollPattern.Pattern);
            scroll.SetScrollPercent(ScrollPattern.NoScroll, 100);
            Wait(() => ((ScrollPattern)Find(modelsScale, "model-table").GetCurrentPattern(ScrollPattern.Pattern)).Current.VerticalScrollPercent > 95,
                "large model table scrolls to the end", process);
            Wait(() => modelsScale.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "ci-extra-0599")) != null,
                "refreshed model table contains the last added model", process);
            var modelChecks = 0;
            Wait(() => (modelChecks = table.FindAll(TreeScope.Descendants, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.CheckBox)).Count) > 0,
                "large model table has painted rows", process);
            if (modelChecks >= 100) throw new InvalidOperationException($"Model table retained {modelChecks} realized checkboxes");
            Capture(modelsScale, Path.Combine(output, "models-600-scrolled.png"));
            Console.WriteLine($"PASS bounded controls: 500 turns / {usageTextNodes} text controls; 600 Fusion models / {fusionChecks} checkboxes; model table / {modelChecks} checkboxes");
            Console.WriteLine("PASS macOS-aligned tray, provider tabs, separate windows and Fusion modes");
            window = OpenWindow(start, process, "providers", "模型与服务");
            handle = (nint)window.Current.NativeWindowHandle;
            PostMessage(handle, 0x112, 0xF060, 0);
            Wait(() => !IsWindowVisible(handle), "close to tray", process);
            using var second = Process.Start(start) ?? throw new InvalidOperationException("Cannot reactivate app");
            Wait(() => IsWindowVisible(handle), "single-instance reactivation", process);
            var secondExited = second.WaitForExit(15000);
            var secondErrors = secondExited ? second.StandardError.ReadToEnd().Trim() : "";
            if (!secondExited || second.ExitCode != 0)
            {
                if (!secondExited) second.Kill(true);
                var state = secondExited ? $"exit {second.ExitCode}" : "timeout";
                throw new InvalidOperationException(
                    $"Second instance did not redirect cleanly ({state})"
                    + (string.IsNullOrEmpty(secondErrors) ? "" : $": {secondErrors}"));
            }
            MoveWindow(handle, desktop.Left + 24, desktop.Top + 24, 960, 600, true);
            Capture(window, Path.Combine(output, "minimum-window.png"));
            Console.WriteLine("PASS close-to-tray, reactivation and provider minimum window");
            process.Refresh();
            var visibleMemory = process.PrivateMemorySize64;
            PostMessage(handle, 0x112, 0xF060, 0);
            Wait(() => !IsWindowVisible(handle), "idle resource sample hides console", process);
            process.Refresh();
            var cpu = process.TotalProcessorTime;
            var idle = Stopwatch.StartNew();
            // Longer than the 30-second refresh interval: catch hidden-window polling.
            Thread.Sleep(TimeSpan.FromSeconds(35));
            process.Refresh();
            File.WriteAllText(Path.Combine(output, "resources.json"), JsonSerializer.Serialize(new
            {
                loaded_modules = process.Modules.Cast<ProcessModule>().Select(module => module.ModuleName).Order().ToArray(),
                background_private_bytes = backgroundPrivateBytes,
                navigation_ms = navigationMs,
                usage_turns = recentTurns.Length,
                usage_text_controls = usageTextNodes,
                fusion_models = 600,
                fusion_checkbox_controls = fusionChecks,
                model_checkbox_controls = modelChecks,
                visible_private_bytes = visibleMemory,
                hidden_private_bytes = process.PrivateMemorySize64,
                hidden_working_set_bytes = process.WorkingSet64,
                idle_sample_seconds = idle.Elapsed.TotalSeconds,
                idle_cpu_seconds = (process.TotalProcessorTime - cpu).TotalSeconds
            }, new JsonSerializerOptions { WriteIndented = true }));
            return 0;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine(error);
            File.WriteAllText(Path.Combine(output, "failure.txt"), error.ToString());
            if (!process.HasExited)
            {
                // Visible text holds the status line and InfoBar, which name the failing CLI step.
                // Every open window is recorded: the failing check may belong to the tray panel.
                var windows = AutomationElement.RootElement.FindAll(TreeScope.Children, new PropertyCondition(AutomationElement.ProcessIdProperty, process.Id)).Cast<AutomationElement>().ToArray();
                for (var index = 0; index < windows.Length; index++)
                {
                    try
                    {
                        var texts = windows[index].FindAll(TreeScope.Descendants, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Text))
                            .Cast<AutomationElement>().Select(element => element.Current.Name).Where(name => !string.IsNullOrWhiteSpace(name)).Take(80);
                        File.AppendAllText(Path.Combine(output, "failure.txt"), $"\n\nVisible text in {windows[index].Current.Name}:\n" + string.Join("\n", texts));
                        Capture(windows[index], Path.Combine(output, $"failure-{index}.png"));
                    }
                    catch (Exception captureError) { Console.Error.WriteLine($"Failure evidence unavailable: {captureError.Message}"); }
                }
            }
            return 1;
        }
        finally
        {
            if (!process.HasExited) { process.Kill(true); process.WaitForExit(); }
            if (gatewayStarted)
                cli.RunAsync(["service", "stop", "--force", "--json"], CancellationToken.None, timeout: TimeSpan.FromSeconds(30)).GetAwaiter().GetResult();
            if (debugger != null)
            {
                if (!debugger.WaitForExit(30000)) debugger.Kill(true);
                debugger.Dispose();
            }
            var errors = nativeErrors.GetAwaiter().GetResult();
            if (!string.IsNullOrWhiteSpace(errors)) File.WriteAllText(Path.Combine(output, "native-errors.txt"), errors);
            var diagnostic = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMixin", "logs", "native-error.txt");
            if (File.Exists(diagnostic)) File.Copy(diagnostic, Path.Combine(output, "native-errors.txt"), true);
            Environment.SetEnvironmentVariable("CODEX_GATEWAY_CONFIG", previousConfig);
            WriteGallery(output);
        }
    }

    private static readonly (string Role, string Title, string Filename)[] Pages =
    [
        ("providers", "模型与服务", "models"),
        ("agents", "Agent 接入管理", "agents"),
        ("context", "上下文", "context"),
        ("fusion", "Fusion 设置", "fusion"),
        ("settings", "设置与诊断", "settings"),
        ("usage", "用量明细", "usage"),
        ("overview", "Codex Mixin · 主面板", "main-panel")
    ];

    private static AutomationElement OpenWindow(ProcessStartInfo source, Process process, string role, string title)
    {
        var start = new ProcessStartInfo(source.FileName) { UseShellExecute = false, RedirectStandardError = true };
        foreach (var variable in source.Environment) start.Environment[variable.Key] = variable.Value;
        start.ArgumentList.Add($"--page={role}");
        using var activation = Process.Start(start) ?? throw new InvalidOperationException("Cannot activate a page");
        AutomationElement? window = null;
        Wait(() =>
        {
            window = AutomationElement.RootElement.FindFirst(TreeScope.Children, new AndCondition(
                new PropertyCondition(AutomationElement.ProcessIdProperty, process.Id),
                new PropertyCondition(AutomationElement.NameProperty, title)));
            return window != null && !window.Current.IsOffscreen;
        }, $"open {title}", process);
        var exited = activation.WaitForExit(15000);
        var errors = exited ? activation.StandardError.ReadToEnd().Trim() : "";
        if (!exited || activation.ExitCode != 0)
        {
            if (!exited) activation.Kill(true);
            var state = exited ? $"exit {activation.ExitCode}" : "timeout";
            throw new InvalidOperationException(
                $"Page activation for {role} did not exit cleanly ({state})"
                + (string.IsNullOrEmpty(errors) ? "" : $": {errors}"));
        }
        if (role != "overview")
            Wait(() => Find(window!, "operation-status").Current.Name.Contains("完成", StringComparison.Ordinal), $"initial load {role}", process);
        return window!;
    }

    private static void SelectTheme(AutomationElement window, string name) => SelectCombo(window, "theme-picker", name);

    private static void SelectCombo(AutomationElement window, string id, string name)
    {
        ((ExpandCollapsePattern)Find(window, id).GetCurrentPattern(ExpandCollapsePattern.Pattern)).Expand();
        var option = window.FindFirst(TreeScope.Descendants, new AndCondition(new PropertyCondition(AutomationElement.NameProperty, name),
            new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.ListItem))) ?? throw new InvalidOperationException($"Option missing: {name}");
        ((SelectionItemPattern)option.GetCurrentPattern(SelectionItemPattern.Pattern)).Select();
        Thread.Sleep(400);
    }

    private static string[] SeedDemo(CodexMixin.CliClient cli, string output, string upstream, int modelCount = 3)
    {
        // This fixture is imported by the real CLI. No provider or gateway result is mocked.
        // Only loopback fixture endpoints are used; all credentials are fictional.
        var models = new[] { "ci-coder", "ci-reasoner", "ci-vision" }
            .Concat(Enumerable.Range(3, modelCount - 3).Select(index => $"ci-extra-{index:0000}")).ToArray();
        var document = new
        {
            config_version = 2,
            gateway_api_key = "ci-gateway-fixture",
            providers = new[] { new
            {
                id = "ci-demo", display_name = "CI 示例供应商", preset_id = "custom", enabled = true,
                protocol = "open_ai_chat", base_url = upstream, api_path = "/v1/chat/completions",
                quota_url = $"{upstream}/quota", quota_parser = "generic",
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
        cli.RunAsync(["config", "import", backup], CancellationToken.None).GetAwaiter().GetResult();
        var options = CodexMixin.Json.Rows(CodexMixin.Json.Field(cli.QueryAsync(["fusion", "models", "--json"], CancellationToken.None).GetAwaiter().GetResult(), "models"));
        var ids = options.Select(row => CodexMixin.Json.Text(row, "id")).ToArray();
        if (ids.Length < 2) throw new InvalidOperationException("Demo configuration did not expose model options");
        var profile = JsonSerializer.Serialize(new
        {
            id = "ci-demo",
            panel_models = ids.Take(3).ToArray(),
            judge_model = ids[0],
            final_model = ids[1],
            min_successful = 1,
            timeout_ms = 300000,
            max_completion_tokens = 2048,
            show_intermediate_results = true,
            panel_tools = new { enabled = true, max_rounds = 16, max_calls_per_model = 64 }
        });
        cli.RunAsync(["fusion", "set", "--profile-json", profile], CancellationToken.None).GetAwaiter().GetResult();
        return ids;
    }

    private static async Task SeedTraffic(string gateway, string[] modelIds, int repetitions = 1)
    {
        using var client = new HttpClient { Timeout = TimeSpan.FromSeconds(10) };
        client.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", "ci-gateway-fixture");
        for (var repetition = 0; repetition < repetitions; repetition++)
            foreach (var model in modelIds.Take(2))
            {
                // Codex calls the Responses endpoint with a session header; the gateway
                // records provider usage only for requests that carry a session.
                using var request = new HttpRequestMessage(HttpMethod.Post, $"{gateway}/v1/responses")
                {
                    Content = JsonContent.Create(new
                    {
                        model,
                        stream = true,
                        instructions = "UI screenshot fixture",
                        input = new[] { new { type = "message", role = "user", content = new[] { new { type = "input_text", text = "say hi" } } } }
                    })
                };
                request.Headers.Add("session-id", $"ui-fixture-{model}-{Guid.NewGuid():N}");
                using var response = await client.SendAsync(request);
                // Reading the whole stream lets the gateway finish and record the request.
                var body = await response.Content.ReadAsStringAsync();
                if (!response.IsSuccessStatusCode || !body.Contains("response.completed", StringComparison.Ordinal))
                    throw new InvalidOperationException($"Demo gateway request failed: {response.StatusCode}: {body}");
            }
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
            "<h1>Codex Mixin · Windows 原生界面</h1><p>由 GitHub Windows runner 运行真实 WinUI 3 应用并截图。供应商和 Fusion 来自 CLI 导入的隔离 fixture；额度来自 loopback HTTP fixture，用量由真实 Rust 网关处理示例请求后记录。所有凭据为虚构值，未连接外部模型服务。</p><main>" + figures + "</main></html>";
        File.WriteAllText(Path.Combine(output, "index.html"), html);
    }

    private static AutomationElement Find(AutomationElement root, string id) =>
        root.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.AutomationIdProperty, id))
        ?? throw new InvalidOperationException($"Control is missing: {id}");

    private static void Assert(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
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

    private static void AssertQuotaTextContrast(AutomationElement window, bool light)
    {
        var bounds = Find(window, "quota-ring-percent").Current.BoundingRectangle;
        using var image = new Bitmap((int)Math.Ceiling(bounds.Width), (int)Math.Ceiling(bounds.Height));
        using (var graphics = Graphics.FromImage(image))
            graphics.CopyFromScreen((int)bounds.Left, (int)bounds.Top, 0, 0, image.Size);
        var contrasting = 0;
        for (var y = 0; y < image.Height; y++)
            for (var x = 0; x < image.Width; x++)
            {
                var color = image.GetPixel(x, y);
                var maximum = Math.Max(color.R, Math.Max(color.G, color.B));
                var minimum = Math.Min(color.R, Math.Min(color.G, color.B));
                if (maximum - minimum < 35 && (light ? maximum < 80 : minimum > 200)) contrasting++;
            }
        if (contrasting < 3) throw new InvalidOperationException($"Quota percentage has no contrasting text in {(light ? "light" : "dark")} mode");
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

    private static void CaptureLower(AutomationElement window, string path)
    {
        var scroller = Find(window, "page-scroll");
        if (!scroller.TryGetCurrentPattern(ScrollPattern.Pattern, out var pattern)) return;
        var scroll = (ScrollPattern)pattern;
        if (!scroll.Current.VerticallyScrollable) return;
        scroll.SetScrollPercent(ScrollPattern.NoScroll, 100);
        Capture(window, path);
        scroll.SetScrollPercent(ScrollPattern.NoScroll, 0);
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct FrameRect { internal int Left; internal int Top; internal int Right; internal int Bottom; }
    [DllImport("dwmapi.dll")]
    private static extern int DwmGetWindowAttribute(nint window, uint attribute, out FrameRect value, int size);

    [DllImport("user32.dll", EntryPoint = "PostMessageW")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool PostMessage(nint window, uint message, nuint wParam, nint lParam);
    private delegate bool EnumWindowProc(nint window, nint parameter);
    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool EnumWindows(EnumWindowProc callback, nint parameter);
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(nint window, out uint process);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool IsZoomed(nint window);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool IsWindowVisible(nint window);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool MoveWindow(nint window, int x, int y, int width, int height, bool repaint);
}
