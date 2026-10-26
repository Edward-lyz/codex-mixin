using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text.Json;
using Microsoft.UI.Dispatching;
using Microsoft.Win32;

[assembly: DefaultDllImportSearchPaths(DllImportSearchPath.AssemblyDirectory | DllImportSearchPath.System32)]

namespace CodexMixin;

// WinSparkle owns download, EdDSA verification and update UI. This adapter only
// connects its native callbacks to our GUI and complete-package installation.
internal sealed class NativeUpdater : IDisposable
{
    private readonly AppUpdates owner;
    private readonly DispatcherQueue dispatcher;
    private readonly QueryCallback canShutdown;
    private readonly InstallCallback runInstaller;
    private readonly NotifyCallback shutdown;
    private readonly NotifyCallback failed;
    private readonly NotifyCallback cancelled;
    private readonly NotifyCallback notFound;
    private bool initialized;
    private bool disposed;
    internal bool Installing { get; private set; }
    private bool gatewayRunning;
    private readonly bool installed = IsInstalled();

    internal NativeUpdater(AppUpdates owner)
    {
        this.owner = owner;
        dispatcher = DispatcherQueue.GetForCurrentThread();
        canShutdown = CanShutdown;
        notFound = () => dispatcher.TryEnqueue(() => owner.FinishUpdate("更新源暂时没有可安装的新版本，请稍后重试。"));
        runInstaller = RunInstaller;
        shutdown = () => dispatcher.TryEnqueue(async () => await ((App)Microsoft.UI.Xaml.Application.Current).ExitAllAsync());
        failed = () => dispatcher.TryEnqueue(() => { Installing = false; ((App)Microsoft.UI.Xaml.Application.Current).SetUpdateInputBlocked(false); owner.FinishUpdate("更新失败；下载、签名或安装前检查未通过，请重试。更新日志位于界面日志目录。"); });
        cancelled = () => dispatcher.TryEnqueue(() => { Installing = false; ((App)Microsoft.UI.Xaml.Application.Current).SetUpdateInputBlocked(false); owner.FinishUpdate("更新已取消，可稍后重试。"); });
    }

    internal void Initialize()
    {
        if (!initialized)
        {
            SetDetails("Edward-lyz", "Codex Mixin", AppUpdates.GuiVersion);
            SetAutomatic(0); // AppUpdates owns the hourly/idle policy.
            SetFeed($"https://github.com/Edward-lyz/codex-mixin/releases/latest/download/appcast-windows-x86_64{(installed ? "" : "-portable")}.xml");
            var key = File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "WinSparkle.PublicKey.txt")).Trim();
            if (SetPublicKey(key) != 1) throw new IOException("Windows 更新签名公钥无效。");
            SetCanShutdown(canShutdown); SetRunInstaller(runInstaller);
            SetShutdown(shutdown); SetError(failed); SetCancelled(cancelled); SetNotFound(notFound);
            Init(); initialized = true;
        }
    }

    internal void Start() { Initialize(); CheckAndInstall(); }

    private int CanShutdown()
    {
        // WinSparkle invokes this on a worker. Never touch WinUI from that thread.
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(25));
        var token = deadline.Token;
        var completion = new TaskCompletionSource<int>(TaskCreationOptions.RunContinuationsAsynchronously);
        if (!dispatcher.TryEnqueue(async () =>
        {
            try
            {
                var permission = await owner.PrepareInstallAsync(token);
                token.ThrowIfCancellationRequested();
                gatewayRunning = permission.Running;
                Installing = permission.Allowed;
                ((App)Microsoft.UI.Xaml.Application.Current).SetUpdateInputBlocked(permission.Allowed);
                if (!permission.Allowed) owner.FinishUpdate(owner.Status);
                completion.TrySetResult(permission.Allowed ? 1 : 0);
            }
            catch (Exception error) { Installing = false; ((App)Microsoft.UI.Xaml.Application.Current).SetUpdateInputBlocked(false); owner.FinishUpdate($"暂缓安装：{error.Message}"); completion.TrySetResult(0); }
        })) return 0;
        try { return completion.Task.WaitAsync(token).GetAwaiter().GetResult(); }
        catch (OperationCanceledException)
        {
            dispatcher.TryEnqueue(() =>
            {
                Installing = false;
                ((App)Microsoft.UI.Xaml.Application.Current).SetUpdateInputBlocked(false);
                owner.FinishUpdate("安装前状态检查超时，请重试。");
            });
            return 0;
        }
    }

    private int RunInstaller(string payload)
    {
        try
        {
            // Keep a verified copy outside WinSparkle's temporary directory;
            // cleanup must not remove it when the application shuts down.
            var directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMixin", "updates", Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(directory);
            var package = Path.Combine(directory, installed ? "update.exe" : "update.zip");
            File.Copy(payload, package);
            var job = Path.Combine(directory, "job.json");
            File.WriteAllText(job, JsonSerializer.Serialize(new
            {
                directory = Path.TrimEndingDirectorySeparator(AppContext.BaseDirectory),
                package,
                installed,
                gatewayRunning,
                pid = Environment.ProcessId
            }));
            var start = new ProcessStartInfo(Path.Combine(Environment.SystemDirectory, "WindowsPowerShell", "v1.0", "powershell.exe"))
            {
                UseShellExecute = false,
                CreateNoWindow = true,
                WorkingDirectory = directory
            };
            foreach (var argument in new[] { "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", Path.Combine(AppContext.BaseDirectory, "install-update.ps1"), "-Job", job }) start.ArgumentList.Add(argument);
            using var helper = Process.Start(start) ?? throw new IOException("无法启动整包更新安装进程。");
            return 1;
        }
        catch (Exception error)
        {
            dispatcher.TryEnqueue(() => { Installing = false; ((App)Microsoft.UI.Xaml.Application.Current).SetUpdateInputBlocked(false); owner.FinishUpdate($"启动安装失败：{error.Message}"); });
            return -1; // WINSPARKLE_RETURN_ERROR; never execute an unchecked file.
        }
    }

    private static bool IsInstalled()
    {
        foreach (var view in new[] { RegistryView.Registry64, RegistryView.Registry32 })
        {
            using var registry = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, view);
            using var key = registry.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexMixin");
            if (key?.GetValue("InstallLocation") is string path &&
                Path.TrimEndingDirectorySeparator(Path.GetFullPath(path)).Equals(Path.TrimEndingDirectorySeparator(AppContext.BaseDirectory), StringComparison.OrdinalIgnoreCase)) return true;
        }
        return false;
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        if (initialized) Cleanup();
        GC.KeepAlive(canShutdown); GC.KeepAlive(runInstaller); GC.KeepAlive(shutdown); GC.KeepAlive(failed); GC.KeepAlive(cancelled); GC.KeepAlive(notFound);
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate int QueryCallback();
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate int InstallCallback([MarshalAs(UnmanagedType.LPWStr)] string path);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate void NotifyCallback();
    private const string Library = "WinSparkle.dll";
    [DllImport(Library, EntryPoint = "win_sparkle_init", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void Init();
    [DllImport(Library, EntryPoint = "win_sparkle_cleanup", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void Cleanup();
    [DllImport(Library, EntryPoint = "win_sparkle_set_app_details", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, CharSet = CharSet.Unicode)] private static extern void SetDetails(string company, string name, string version);
    [DllImport(Library, EntryPoint = "win_sparkle_set_appcast_url", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void SetFeed([MarshalAs(UnmanagedType.LPUTF8Str)] string url);
    [DllImport(Library, EntryPoint = "win_sparkle_set_eddsa_public_key", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern int SetPublicKey([MarshalAs(UnmanagedType.LPUTF8Str)] string key);
    [DllImport(Library, EntryPoint = "win_sparkle_set_automatic_check_for_updates", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void SetAutomatic(int enabled);
    [DllImport(Library, EntryPoint = "win_sparkle_check_update_with_ui_and_install", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void CheckAndInstall();
    [DllImport(Library, EntryPoint = "win_sparkle_set_can_shutdown_callback", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void SetCanShutdown(QueryCallback callback);
    [DllImport(Library, EntryPoint = "win_sparkle_set_user_run_installer_callback", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void SetRunInstaller(InstallCallback callback);
    [DllImport(Library, EntryPoint = "win_sparkle_set_shutdown_request_callback", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void SetShutdown(NotifyCallback callback);
    [DllImport(Library, EntryPoint = "win_sparkle_set_error_callback", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void SetError(NotifyCallback callback);
    [DllImport(Library, EntryPoint = "win_sparkle_set_did_not_find_update_callback", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void SetNotFound(NotifyCallback callback);
    [DllImport(Library, EntryPoint = "win_sparkle_set_update_cancelled_callback", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)] private static extern void SetCancelled(NotifyCallback callback);
}
