using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32;

namespace CodexMixin;

internal sealed class NativeShell : IDisposable
{
    private const uint TrayMessage = 0x8001;
    private const uint NotifyId = 1;
    private readonly nint window;
    private readonly SubclassProc procedure;
    private readonly Action<string> command;
    private readonly uint taskbarCreated;
    private readonly nint icon;
    private bool disposed;

    internal NativeShell(nint window, Action<string> command)
    {
        this.window = window;
        this.command = command;
        procedure = WindowProc;
        taskbarCreated = RegisterWindowMessage("TaskbarCreated");
        icon = LoadImage(0, Path.Combine(AppContext.BaseDirectory, "Assets", "tray_icon.ico"), 1, 0, 0, 0x10);
        if (icon == 0) throw new Win32Exception(Marshal.GetLastWin32Error(), "无法加载托盘图标，请生成 Windows assets。");
        if (!SetWindowSubclass(window, procedure, 1, 0))
        {
            DestroyIcon(icon);
            throw new Win32Exception(Marshal.GetLastWin32Error(), "无法安装原生窗口事件。");
        }
        try { AddIcon(); }
        catch { RemoveWindowSubclass(window, procedure, 1); DestroyIcon(icon); throw; }
    }

    private NotifyIconData IconData() => new()
    {
        Size = (uint)Marshal.SizeOf<NotifyIconData>(), Window = window, Id = NotifyId,
        Flags = 1 | 2 | 4, Callback = TrayMessage, Icon = icon, Tip = "Codex Mixin",
        Info = "", InfoTitle = ""
    };

    private void AddIcon()
    {
        var value = IconData();
        if (!ShellNotifyIcon(0, ref value))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "无法创建系统托盘图标。");
    }

    internal void Show()
    {
        ShowWindow(window, 9);
        SetForegroundWindow(window);
    }

    internal void Hide() => ShowWindow(window, 0);

    internal static Windows.Graphics.RectInt32 PanelBounds()
    {
        GetCursorPos(out var position);
        var monitor = MonitorFromPoint(position, 2);
        var info = new MonitorInfo { Size = (uint)Marshal.SizeOf<MonitorInfo>() };
        if (!GetMonitorInfo(monitor, ref info)) throw new Win32Exception(Marshal.GetLastWin32Error());
        // AppWindow bounds are physical pixels; the panel layout is designed in 96-DPI units.
        var result = GetDpiForMonitor(monitor, 0, out var dpi, out _);
        if (result != 0) throw new Win32Exception(result);
        var scale = dpi / 96.0;
        var margin = (int)(8 * scale);
        var width = Math.Min((int)(360 * scale), info.Work.Right - info.Work.Left - 2 * margin);
        var height = Math.Min((int)(640 * scale), info.Work.Bottom - info.Work.Top - 2 * margin);
        return new Windows.Graphics.RectInt32(Math.Clamp(position.X - width / 2, info.Work.Left + margin, info.Work.Right - width - margin), info.Work.Bottom - height - margin, width, height);
    }

    private nint WindowProc(nint hwnd, uint message, nuint wParam, nint lParam, nuint id, nuint reference)
    {
        if (message == taskbarCreated)
        {
            try { AddIcon(); }
            catch (Win32Exception) { command("tray-error"); }
        }
        if (message == 0x24) // WM_GETMINMAXINFO: keep the navigation usable at every DPI.
        {
            var limits = Marshal.PtrToStructure<MinMaxInfo>(lParam);
            var scale = GetDpiForWindow(window) / 96.0;
            limits.MinTrack = new Point { X = (int)(760 * scale), Y = (int)(560 * scale) };
            Marshal.StructureToPtr(limits, lParam, false);
            return 0;
        }
        if (message == TrayMessage)
        {
            if ((uint)lParam is 0x202 or 0x203) command("overview");
            if ((uint)lParam == 0x205) ShowMenu();
            return 0;
        }
        return DefSubclassProc(hwnd, message, wParam, lParam);
    }

    private void ShowMenu()
    {
        var menu = CreatePopupMenu();
        if (menu == 0) { command("tray-error"); return; }
        try
        {
            AppendMenu(menu, 0, 1, "模型与服务…");
            AppendMenu(menu, 0, 2, "启动网关");
            AppendMenu(menu, 0, 3, "停止网关");
            AppendMenu(menu, 0x800, 0, "");
            AppendMenu(menu, 0, 4, "设置");
            AppendMenu(menu, 0, 5, "退出应用");
            AppendMenu(menu, 0, 6, "Agent 接入管理…");
            AppendMenu(menu, 0, 7, "Fusion 设置…");
            GetCursorPos(out var position);
            SetForegroundWindow(window);
            var selected = TrackPopupMenu(menu, 0x100 | 0x2, position.X, position.Y, 0, window, 0);
            PostMessage(window, 0, 0, 0);
            if (selected != 0) command(selected switch
            {
                1 => "providers", 2 => "start", 3 => "stop", 4 => "settings", 6 => "agents", 7 => "fusion", _ => "exit"
            });
        }
        finally { DestroyMenu(menu); }
    }

    internal static bool StartupEnabled()
    {
        using var run = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run");
        return run?.GetValue("CodexMixin") is string;
    }

    internal static bool StartupBlocked()
    {
        using var approved = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run");
        return approved?.GetValue("CodexMixin") is byte[] { Length: > 0 } value && value[0] is 3 or 7;
    }

    internal static void SetStartup(bool enabled)
    {
        using var run = Registry.CurrentUser.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run", true);
        if (!enabled) { run.DeleteValue("CodexMixin", false); return; }
        if (StartupBlocked()) throw new InvalidOperationException("启动项已被 Windows 禁用，请在系统「启动应用」中启用。");
        var path = Environment.ProcessPath ?? throw new InvalidOperationException("无法定位应用程序。");
        run.SetValue("CodexMixin", $"\"{path}\" --background");
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        var value = IconData();
        ShellNotifyIcon(2, ref value);
        RemoveWindowSubclass(window, procedure, 1);
        DestroyIcon(icon);
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Point { internal int X; internal int Y; }
    [StructLayout(LayoutKind.Sequential)]
    private struct Rect { internal int Left; internal int Top; internal int Right; internal int Bottom; }
    [StructLayout(LayoutKind.Sequential)]
    private struct MonitorInfo { internal uint Size; internal Rect Monitor; internal Rect Work; internal uint Flags; }
    [StructLayout(LayoutKind.Sequential)]
    private struct MinMaxInfo { internal Point Reserved; internal Point MaxSize; internal Point MaxPosition; internal Point MinTrack; internal Point MaxTrack; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NotifyIconData
    {
        internal uint Size; internal nint Window; internal uint Id; internal uint Flags; internal uint Callback; internal nint Icon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] internal string Tip;
        internal uint State; internal uint StateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] internal string Info;
        internal uint Version;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] internal string InfoTitle;
        internal uint InfoFlags; internal Guid Guid; internal nint BalloonIcon;
    }
    private delegate nint SubclassProc(nint window, uint message, nuint wParam, nint lParam, nuint id, nuint reference);
    [DllImport("comctl32.dll", SetLastError = true)] [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowSubclass(nint window, SubclassProc procedure, nuint id, nuint reference);
    [DllImport("comctl32.dll")] [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool RemoveWindowSubclass(nint window, SubclassProc procedure, nuint id);
    [DllImport("comctl32.dll")] private static extern nint DefSubclassProc(nint window, uint message, nuint wParam, nint lParam);
    [DllImport("shell32.dll", EntryPoint = "Shell_NotifyIconW", CharSet = CharSet.Unicode, SetLastError = true)] [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ShellNotifyIcon(uint message, ref NotifyIconData value);
    [DllImport("user32.dll", EntryPoint = "LoadImageW", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern nint LoadImage(nint instance, string name, uint type, int width, int height, uint flags);
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool DestroyIcon(nint icon);
    [DllImport("user32.dll")] private static extern uint GetDpiForWindow(nint window);
    [DllImport("user32.dll")] private static extern nint MonitorFromPoint(Point point, uint flags);
    [DllImport("shcore.dll")] private static extern int GetDpiForMonitor(nint monitor, int type, out uint dpiX, out uint dpiY);
    [DllImport("user32.dll", EntryPoint = "GetMonitorInfoW", SetLastError = true)] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool GetMonitorInfo(nint monitor, ref MonitorInfo info);
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool ShowWindow(nint window, int command);
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool SetForegroundWindow(nint window);
    [DllImport("user32.dll", EntryPoint = "RegisterWindowMessageW", CharSet = CharSet.Unicode)] private static extern uint RegisterWindowMessage(string name);
    [DllImport("user32.dll")] private static extern nint CreatePopupMenu();
    [DllImport("user32.dll", EntryPoint = "AppendMenuW", CharSet = CharSet.Unicode)] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool AppendMenu(nint menu, uint flags, nuint id, string text);
    [DllImport("user32.dll")] private static extern uint TrackPopupMenu(nint menu, uint flags, int x, int y, int reserved, nint window, nint rect);
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool GetCursorPos(out Point position);
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool DestroyMenu(nint menu);
    [DllImport("user32.dll", EntryPoint = "PostMessageW")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool PostMessage(nint window, uint message, nuint wParam, nint lParam);
}
