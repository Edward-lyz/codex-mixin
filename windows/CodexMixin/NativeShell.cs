using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32;

namespace CodexMixin;

internal sealed class NativeShell : IDisposable
{
    private const uint TrayMessage = 0x8001;
    private const uint NotifyId = 1;
    private const uint NotifyIconVersion = 4;
    private static readonly Guid TrayGuid = new("3d00e42d-5ac5-49f9-b8bf-eabec79ad9f1");
    private enum TaskbarEdge { Left, Right, Top, Bottom }
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
        if (icon == 0) throw new Win32Exception(Marshal.GetLastWin32Error(), Localized.Get("无法加载托盘图标，请生成 Windows assets。"));
        if (!SetWindowSubclass(window, procedure, 1, 0))
        {
            DestroyIcon(icon);
            throw new Win32Exception(Marshal.GetLastWin32Error(), Localized.Get("无法安装原生窗口事件。"));
        }
        try { AddIcon(); }
        catch { RemoveWindowSubclass(window, procedure, 1); DestroyIcon(icon); throw; }
    }

    private NotifyIconData IconData() => new()
    {
        Size = (uint)Marshal.SizeOf<NotifyIconData>(),
        Window = window,
        Id = NotifyId,
        Flags = 1 | 2 | 4 | 0x20 | 0x80, // Include GUID identity and the v4 tooltip.
        Guid = TrayGuid,
        Callback = TrayMessage,
        Icon = icon,
        Tip = "Codex Mixin",
        Info = "",
        InfoTitle = ""
    };

    private void AddIcon()
    {
        var value = IconData();
        if (!ShellNotifyIcon(0, ref value))
            throw new Win32Exception(Marshal.GetLastWin32Error(), Localized.Get("无法创建系统托盘图标。"));
        value.Version = NotifyIconVersion;
        if (!ShellNotifyIcon(4, ref value)) // NIM_SETVERSION after every NIM_ADD.
        {
            var error = Marshal.GetLastWin32Error();
            ShellNotifyIcon(2, ref value);
            throw new Win32Exception(error, Localized.Get("无法启用系统托盘交互协议。"));
        }
    }

    internal void Show()
    {
        // SW_RESTORE also unmaximizes a visible window. Only restore minimized windows.
        ShowWindow(window, IsIconic(window) ? 9 : 5);
        SetForegroundWindow(window);
    }

    internal bool NotifyUpdate(string version)
    {
        var notification = IconData();
        notification.Flags = 0x10 | 0x20; // NIF_INFO
        notification.InfoTitle = Localized.Get("Codex Mixin 有可用更新");
        notification.Info = Localized.Format("发现 {0}，单击查看版本并下载完整应用包。", version);
        notification.InfoFlags = 1;
        return ShellNotifyIcon(1, ref notification);
    }


    // Window bounds are physical pixels; WPF positions in DIPs per monitor.
    internal static void Place(nint handle, System.Drawing.Rectangle bounds)
    {
        if (!SetWindowPos(handle, 0, bounds.X, bounds.Y, bounds.Width, bounds.Height, 0x14))
            throw new Win32Exception(Marshal.GetLastWin32Error());
    }

    internal static void ConfigurePopupFrame(nint handle)
    {
        // Borderless popups keep no caption or sizing frame.
        const int windowStyle = -16;
        const long captionAndSizingFrame = 0x00c40000;
        var style = GetWindowLongPtr(handle, windowStyle);
        if (SetWindowLongPtr(handle, windowStyle, (nint)(style.ToInt64() & ~captionAndSizingFrame)) == 0)
            throw new Win32Exception(Marshal.GetLastWin32Error());
        if (!SetWindowPos(handle, 0, 0, 0, 0, 0, 0x37))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        if (!OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22000)) return;
        // Windows 11 rounds the popup like a system flyout and draws its border along the curve.
        const int cornerPreferenceAttribute = 33;
        var round = 2u;
        Marshal.ThrowExceptionForHR(DwmSetWindowAttribute(handle, cornerPreferenceAttribute, ref round, sizeof(uint)));
    }

    internal static System.Drawing.Rectangle ConsoleBounds() => CenteredBounds(1440, 960, 0.92);

    // Side windows fill at most the work area so their lower controls stay on screen.
    internal static System.Drawing.Rectangle SideWindowBounds() => CenteredBounds(1120, 800, 1);

    private static System.Drawing.Rectangle CenteredBounds(int preferredWidth, int preferredHeight, double fill)
    {
        GetCursorPos(out var position);
        var monitor = MonitorFromPoint(position, 2);
        var info = new MonitorInfo { Size = (uint)Marshal.SizeOf<MonitorInfo>() };
        if (!GetMonitorInfo(monitor, ref info)) throw new Win32Exception(Marshal.GetLastWin32Error());
        Marshal.ThrowExceptionForHR(GetDpiForMonitor(monitor, 0, out var dpi, out _));
        var scale = dpi / 96.0;
        var width = Math.Min((int)(preferredWidth * scale), (int)((info.Work.Right - info.Work.Left) * fill));
        var height = Math.Min((int)(preferredHeight * scale), (int)((info.Work.Bottom - info.Work.Top) * fill));
        return new System.Drawing.Rectangle(info.Work.Left + (info.Work.Right - info.Work.Left - width) / 2,
            info.Work.Top + (info.Work.Bottom - info.Work.Top - height) / 2, width, height);
    }

    private static Rect TrayAnchor()
    {
        var identifier = new NotifyIconIdentifier
        {
            Size = (uint)Marshal.SizeOf<NotifyIconIdentifier>(),
            Window = 0,
            Id = 0,
            Guid = TrayGuid
        };
        if (ShellNotifyIconGetRect(ref identifier, out var bounds) == 0) return bounds;
        // Explorer may hide the icon in its overflow menu. Use the invocation
        // point until the Shell can supply its rectangle again.
        if (!GetCursorPos(out var cursor)) throw new Win32Exception(Marshal.GetLastWin32Error());
        return new Rect { Left = cursor.X, Right = cursor.X, Top = cursor.Y, Bottom = cursor.Y };
    }

    internal static System.Drawing.Rectangle PanelBounds(int? requestedHeight = null)
    {
        var anchor = TrayAnchor();
        var position = new Point { X = (anchor.Left + anchor.Right) / 2, Y = (anchor.Top + anchor.Bottom) / 2 };
        var monitor = MonitorFromPoint(position, 2);
        var info = new MonitorInfo { Size = (uint)Marshal.SizeOf<MonitorInfo>() };
        if (!GetMonitorInfo(monitor, ref info)) throw new Win32Exception(Marshal.GetLastWin32Error());
        // Window bounds are physical pixels, while the layout uses 96-DPI units.
        Marshal.ThrowExceptionForHR(GetDpiForMonitor(monitor, 0, out var dpi, out _));
        var margin = (int)(8 * dpi / 96.0);
        var width = Math.Min((int)(360 * dpi / 96.0), info.Work.Right - info.Work.Left - 2 * margin);
        var height = Math.Min(requestedHeight ?? int.MaxValue, Math.Min((int)(720 * dpi / 96.0), info.Work.Bottom - info.Work.Top - 2 * margin));
        var edge = new[]
        {
            (Distance: Math.Abs(position.X - info.Work.Left), Edge: TaskbarEdge.Left),
            (Distance: Math.Abs(position.X - info.Work.Right), Edge: TaskbarEdge.Right),
            (Distance: Math.Abs(position.Y - info.Work.Top), Edge: TaskbarEdge.Top),
            (Distance: Math.Abs(position.Y - info.Work.Bottom), Edge: TaskbarEdge.Bottom)
        }.MinBy(candidate => candidate.Distance).Edge;
        var x = edge switch
        {
            TaskbarEdge.Left => info.Work.Left + margin,
            TaskbarEdge.Right => info.Work.Right - width - margin,
            _ => position.X - width / 2
        };
        var y = edge switch
        {
            TaskbarEdge.Top => info.Work.Top + margin,
            TaskbarEdge.Bottom => info.Work.Bottom - height - margin,
            _ => position.Y - height / 2
        };
        return new System.Drawing.Rectangle(
            Math.Clamp(x, info.Work.Left + margin, info.Work.Right - width - margin),
            Math.Clamp(y, info.Work.Top + margin, info.Work.Bottom - height - margin), width, height);
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
            var notification = unchecked((uint)lParam.ToInt64()) & 0xffff;
            if (notification is 0x400 or 0x401 or 0x7b) command("overview"); // Left, keyboard, and right click share the themed panel.
            if (notification == 0x405) command("updates"); // NIN_BALLOONUSERCLICK
            return 0;
        }
        return DefSubclassProc(hwnd, message, wParam, lParam);
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
        if (StartupBlocked()) throw new InvalidOperationException(Localized.Get("启动项已被 Windows 禁用，请在系统「启动应用」中启用。"));
        var path = Environment.ProcessPath ?? throw new InvalidOperationException(Localized.Get("无法定位应用程序。"));
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
    [StructLayout(LayoutKind.Sequential)]
    private struct NotifyIconIdentifier { internal uint Size; internal nint Window; internal uint Id; internal Guid Guid; }
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
    [DllImport("comctl32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowSubclass(nint window, SubclassProc procedure, nuint id, nuint reference);
    [DllImport("comctl32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool RemoveWindowSubclass(nint window, SubclassProc procedure, nuint id);
    [DllImport("comctl32.dll")] private static extern nint DefSubclassProc(nint window, uint message, nuint wParam, nint lParam);
    [DllImport("shell32.dll", EntryPoint = "Shell_NotifyIconW", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ShellNotifyIcon(uint message, ref NotifyIconData value);
    [DllImport("shell32.dll", EntryPoint = "Shell_NotifyIconGetRect")]
    private static extern int ShellNotifyIconGetRect(ref NotifyIconIdentifier identifier, out Rect bounds);
    [DllImport("user32.dll", EntryPoint = "LoadImageW", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern nint LoadImage(nint instance, string name, uint type, int width, int height, uint flags);
    [DllImport("user32.dll")][return: MarshalAs(UnmanagedType.Bool)] private static extern bool DestroyIcon(nint icon);
    [DllImport("user32.dll")] private static extern uint GetDpiForWindow(nint window);
    [DllImport("user32.dll")] private static extern nint MonitorFromPoint(Point point, uint flags);
    [DllImport("shcore.dll")] private static extern int GetDpiForMonitor(nint monitor, int type, out uint dpiX, out uint dpiY);
    [DllImport("user32.dll", EntryPoint = "GetMonitorInfoW", SetLastError = true)][return: MarshalAs(UnmanagedType.Bool)] private static extern bool GetMonitorInfo(nint monitor, ref MonitorInfo info);
    [DllImport("user32.dll")][return: MarshalAs(UnmanagedType.Bool)] private static extern bool IsIconic(nint window);
    [DllImport("user32.dll")][return: MarshalAs(UnmanagedType.Bool)] private static extern bool ShowWindow(nint window, int command);
    [DllImport("user32.dll")][return: MarshalAs(UnmanagedType.Bool)] private static extern bool SetForegroundWindow(nint window);
    [DllImport("user32.dll", EntryPoint = "RegisterWindowMessageW", CharSet = CharSet.Unicode)] private static extern uint RegisterWindowMessage(string name);
    [DllImport("user32.dll")][return: MarshalAs(UnmanagedType.Bool)] private static extern bool GetCursorPos(out Point position);
    [DllImport("dwmapi.dll")] private static extern int DwmSetWindowAttribute(nint window, int attribute, ref uint value, int size);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW", SetLastError = true)] private static extern nint GetWindowLongPtr(nint window, int index);
    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW", SetLastError = true)] private static extern nint SetWindowLongPtr(nint window, int index, nint value);
    [DllImport("user32.dll", SetLastError = true)][return: MarshalAs(UnmanagedType.Bool)] private static extern bool SetWindowPos(nint window, nint after, int x, int y, int width, int height, uint flags);
}
