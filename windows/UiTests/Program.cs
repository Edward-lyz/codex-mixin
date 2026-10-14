using System;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;
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
        using var process = Process.Start(start) ?? throw new InvalidOperationException("Cannot launch WinUI app");
        try
        {
            nint handle = 0;
            Wait(() => { process.Refresh(); handle = process.MainWindowHandle; return handle != 0; }, "native main window", process);
            var window = AutomationElement.FromHandle(handle);
            Wait(() => Find(window, "operation-status").Current.Name.Contains("完成", StringComparison.Ordinal), "initial CLI load", process);
            AssertName(window, "本地网关 · 未配置");
            Console.WriteLine("PASS fresh installation shows unconfigured gateway");
            Capture(window, Path.Combine(output, "overview.png"));
            Navigate(window, "nav-providers", "管理服务连接与向 Agent 提供的模型", process);
            AssertName(window, "暂无供应商");
            Console.WriteLine("PASS provider empty state");
            Navigate(window, "nav-agents", "检测、接入与升级由本地 CLI 执行", process);
            AssertName(window, "检查版本更新");
            Console.WriteLine("PASS agent contract reaches native controls");
            Navigate(window, "nav-usage", "真实请求用量、缓存命中与供应商额度", process);
            AssertName(window, "用量不可用");
            Console.WriteLine("PASS stopped gateway does not show zero usage");
            Navigate(window, "nav-fusion", "组合 Panel、Judge 与 Final 模型", process);
            AssertName(window, "暂无可用模型");
            Console.WriteLine("PASS Fusion empty state");
            Navigate(window, "nav-settings", "外观、启动、备份与诊断", process);
            var theme = Find(window, "theme-picker");
            ((ExpandCollapsePattern)theme.GetCurrentPattern(ExpandCollapsePattern.Pattern)).Expand();
            var dark = window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "深色"));
            if (dark == null) throw new InvalidOperationException("Dark theme option is missing");
            ((SelectionItemPattern)dark.GetCurrentPattern(SelectionItemPattern.Pattern)).Select();
            Console.WriteLine("PASS native theme selection");
            Capture(window, Path.Combine(output, "settings-dark.png"));
            // WM_SYSCOMMAND/SC_CLOSE must hide the shell without terminating it.
            PostMessage(handle, 0x112, 0xF060, 0);
            Wait(() => !IsWindowVisible(handle), "close to tray", process);
            using var second = Process.Start(start) ?? throw new InvalidOperationException("Cannot reactivate app");
            Wait(() => IsWindowVisible(handle), "single-instance reactivation", process);
            if (!second.WaitForExit(15000)) throw new InvalidOperationException("Second instance did not redirect activation");
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
            return 1;
        }
        finally
        {
            if (!process.HasExited) { process.Kill(true); process.WaitForExit(); }
        }
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
        var rect = window.Current.BoundingRectangle;
        using var image = new Bitmap((int)rect.Width, (int)rect.Height);
        using var graphics = Graphics.FromImage(image);
        graphics.CopyFromScreen((int)rect.Left, (int)rect.Top, 0, 0, image.Size);
        image.Save(path, ImageFormat.Png);
    }

    [DllImport("user32.dll", EntryPoint = "PostMessageW")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool PostMessage(nint window, uint message, nuint wParam, nint lParam);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool IsWindowVisible(nint window);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool MoveWindow(nint window, int x, int y, int width, int height, bool repaint);
}
