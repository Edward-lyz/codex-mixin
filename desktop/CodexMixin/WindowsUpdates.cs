#if WINDOWS
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using System.Text.Json;
using Avalonia.Threading;
using Microsoft.Win32;

[assembly: DefaultDllImportSearchPaths(DllImportSearchPath.AssemblyDirectory | DllImportSearchPath.System32)]

namespace CodexMixin;

// The GUI owns idle scheduling; WinSparkle owns signed complete-package updates.
[SupportedOSPlatform("windows")]
internal sealed class AppUpdates(Func<CliClient> client)
{
    private const string SettingsKey = @"Software\CodexMixin\Updates";
    private readonly CancellationTokenSource lifetime = new();
    private Task active = Task.CompletedTask;
    private NativeUpdater? updater;
    private bool checking;
    private bool updating;
    private bool automaticInstall;
    internal bool LastInstallFailed { get; private set; }
    internal bool Installing => updater?.Installing == true;
    internal static string GuiVersion => (typeof(App).Assembly.GetName().Version
        ?? throw new InvalidOperationException(Localized.Get("无法读取 GUI 版本。"))).ToString(3);
    internal event Action? Changed;
    internal AppRelease? Release { get; private set; }
    internal bool Busy => checking || updating;
    internal string Status { get; private set; } = Localized.Get("尚未检查更新。");
    internal bool Automatic { get; private set; } = true;

    internal void LoadPreferences()
    {
        try
        {
            using var settings = Registry.CurrentUser.OpenSubKey(SettingsKey);
            Automatic = settings?.GetValue("Automatic") is not int enabled || enabled != 0;
            if (settings?.GetValue("LastInstallError") is string error && error.Length > 0)
            {
                LastInstallFailed = true;
                Status = Localized.Format("上次更新失败：{0}", error);
            }
            updater = new NativeUpdater(this);
            updater.Initialize();
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or System.Security.SecurityException or DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
        {
            Automatic = false;
            Status = Localized.Format("更新器初始化失败：{0}", error.Message);
        }
    }

    internal void SetAutomatic(bool enabled)
    {
        using var settings = Registry.CurrentUser.CreateSubKey(SettingsKey, true);
        settings.SetValue("Automatic", enabled ? 1 : 0);
        Automatic = enabled;
        Changed?.Invoke();
    }

    internal Task CheckAsync(bool automatic = false)
    {
        if (Busy || lifetime.IsCancellationRequested || automatic && !Automatic) return active;
        active = CheckCoreAsync(automatic);
        return active;
    }

    private async Task CheckCoreAsync(bool automatic)
    {
        var previousStatus = Status;
        checking = true;
        Status = Localized.Get("正在检查 Windows 完整应用更新…");
        Changed?.Invoke();
        try
        {
            using var settings = Registry.CurrentUser.CreateSubKey(SettingsKey, true);
            if (automatic && settings.GetValue("NextCheck") is string next
                && DateTimeOffset.TryParse(next, out var nextCheck) && nextCheck > DateTimeOffset.UtcNow)
            {
                Status = previousStatus;
                return;
            }
            // Persist before the query so restarts cannot trigger a check loop.
            settings.SetValue("NextCheck", DateTimeOffset.UtcNow.AddHours(1).ToString("O"));
            var snapshot = await client().QueryAsync(["update", "--check", "--json"], lifetime.Token, TimeSpan.FromSeconds(60));
            Release = AppRelease.Parse(snapshot, GuiVersion);
            Status = Release.IsNewer
                ? Localized.Format("发现 {0}。", Release.Version)
                : Localized.Format("当前安装无需升级到发布版 {0}。", Release.Version);
            if (!Release.IsNewer) return;
            automaticInstall = automatic;
            if (!(await PrepareInstallAsync(lifetime.Token)).Allowed) return;
            var attempts = settings.GetValue("AttemptVersion") as string == Release.Version && settings.GetValue("Attempts") is int count ? count : 0;
            if (attempts < 0) throw new CliFailure(Localized.Get("更新重试计数无效，请检查界面更新设置。"));
            if (automatic && attempts >= 3) { Status = Localized.Get("自动更新已重试三次，请点立即更新重试。"); return; }
            settings.SetValue("AttemptVersion", Release.Version);
            settings.SetValue("Attempts", Math.Min(attempts, 2) + 1);
            settings.SetValue("NextCheck", DateTimeOffset.UtcNow.AddHours(Math.Min(4, 1 << Math.Min(attempts, 2))).ToString("O"));
            updater ??= new NativeUpdater(this);
            updating = true;
            Status = Localized.Format("正在更新到 {0}：下载和验签由 WinSparkle 处理，完成后自动安装并重启。", Release.Version);
            updater.Start(Release.CurrentVersion);
        }
        catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { Status = Localized.Get("更新检查已取消。"); }
        catch (Exception error)
        {
            updating = false;
            Status = Localized.Format("更新失败：{0} 可点立即更新重试。", error.Message);
            await Diagnostics.RecordAsync("updates", $"{DateTimeOffset.UtcNow:O} stage=metadata outcome=failed error_type={error.GetType().Name}");
        }
        finally { checking = false; Changed?.Invoke(); }
    }

    internal async Task<(bool Allowed, bool Running)> PrepareInstallAsync(CancellationToken token)
    {
        var app = (App)Avalonia.Application.Current!;
        if (app.HasPendingChanges || app.HasDraftChanges)
        {
            Status = Localized.Get("暂缓更新：请先保存草稿并完成当前操作。");
            Changed?.Invoke();
            return (false, false);
        }
        var status = await client().QueryAsync(["service", "status", "--json"], token, TimeSpan.FromSeconds(20));
        _ = Json.Gateway(status); // Reject unknown service states before installing.
        var running = Json.Flag(status, "configured") && Json.Text(status, "gateway") == "running";
        if (running)
        {
            var activity = Json.Field(status, "activity");
            var activeRequests = Json.Integer(activity, "active_requests");
            var idle = Json.OptionalInteger(activity, "idle_seconds");
            if (activeRequests != 0 || automaticInstall && idle is not >= 1800)
            {
                Status = automaticInstall ? Localized.Get("等待网关空闲 30 分钟后自动更新。") : Localized.Get("正在处理请求，完成后请再点立即更新。");
                Changed?.Invoke();
                return (false, true);
            }
        }
        token.ThrowIfCancellationRequested();
        // The second check covers edits made while the live status was loading.
        return (!app.HasPendingChanges && !app.HasDraftChanges, running);
    }

    internal void FinishUpdate(string status)
    {
        updating = false;
        Status = status;
        Changed?.Invoke();
        _ = Diagnostics.RecordAsync("updates", $"{DateTimeOffset.UtcNow:O} outcome=not_installed");
    }

    internal async Task StopAsync()
    {
        lifetime.Cancel();
        await active;
        updater?.Dispose();
        lifetime.Dispose();
    }
}

// WinSparkle owns download, EdDSA verification and update UI. This adapter only
// connects its native callbacks to our GUI and complete-package installation.
[SupportedOSPlatform("windows")]
internal sealed class NativeUpdater : IDisposable
{
    private readonly AppUpdates owner;
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

    private static App Shell => (App)Avalonia.Application.Current!;

    internal NativeUpdater(AppUpdates owner)
    {
        this.owner = owner;
        canShutdown = CanShutdown;
        runInstaller = RunInstaller;
        notFound = () => Dispatcher.UIThread.Post(() => owner.FinishUpdate(Localized.Get("更新源暂时没有可安装的新版本，请稍后重试。")));
        shutdown = () => Dispatcher.UIThread.Post(async () => await Shell.ExitAllAsync());
        failed = () => Dispatcher.UIThread.Post(() => Abort(Localized.Get("更新失败；下载、签名或安装前检查未通过，请重试。更新日志位于界面日志目录。")));
        cancelled = () => Dispatcher.UIThread.Post(() => Abort(Localized.Get("更新已取消，可稍后重试。")));
    }

    // Callbacks arrive on WinSparkle threads; the UI thread restores input and reports.
    private void Abort(string status)
    {
        Installing = false;
        Shell.SetUpdateInputBlocked(false);
        owner.FinishUpdate(status);
    }

    internal void Initialize()
    {
        if (initialized) return;
        SetDetails("Edward-lyz", "Codex Mixin", AppUpdates.GuiVersion);
        SetBuildVersion(AppUpdates.GuiVersion);
        SetAutomatic(0); // AppUpdates owns the hourly/idle policy.
        SetFeed($"https://github.com/Edward-lyz/codex-mixin/releases/latest/download/appcast-windows-x86_64{(installed ? "" : "-portable")}.xml");
        var key = File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "WinSparkle.PublicKey.txt")).Trim();
        if (SetPublicKey(key) != 1) throw new IOException(Localized.Get("Windows 更新签名公钥无效。"));
        SetCanShutdown(canShutdown); SetRunInstaller(runInstaller);
        SetShutdown(shutdown); SetError(failed); SetCancelled(cancelled); SetNotFound(notFound);
        Init(); initialized = true;
    }

    internal void Start(string currentVersion) { Initialize(); SetBuildVersion(currentVersion); CheckAndInstall(); }

    private int CanShutdown()
    {
        // WinSparkle invokes this on a worker. Never touch UI state from that thread.
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(25));
        var token = deadline.Token;
        var completion = new TaskCompletionSource<int>(TaskCreationOptions.RunContinuationsAsynchronously);
        _ = Dispatcher.UIThread.InvokeAsync(async () =>
        {
            try
            {
                var permission = await owner.PrepareInstallAsync(token);
                token.ThrowIfCancellationRequested();
                gatewayRunning = permission.Running;
                Installing = permission.Allowed;
                Shell.SetUpdateInputBlocked(permission.Allowed);
                if (!permission.Allowed) owner.FinishUpdate(owner.Status);
                completion.TrySetResult(permission.Allowed ? 1 : 0);
            }
            catch (Exception error)
            {
                Abort(Localized.Format("暂缓安装：{0}", error.Message));
                completion.TrySetResult(0);
            }
        });
        try { return completion.Task.WaitAsync(token).GetAwaiter().GetResult(); }
        catch (OperationCanceledException)
        {
            Dispatcher.UIThread.Post(() => Abort(Localized.Get("安装前状态检查超时，请重试。")));
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
            var start = new System.Diagnostics.ProcessStartInfo(Path.Combine(Environment.SystemDirectory, "WindowsPowerShell", "v1.0", "powershell.exe"))
            {
                UseShellExecute = false,
                CreateNoWindow = true,
                WorkingDirectory = directory
            };
            start.Environment.Remove("PSModulePath");
            foreach (var argument in new[] { "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", Path.Combine(AppContext.BaseDirectory, "install-update.ps1"), "-Job", job }) start.ArgumentList.Add(argument);
            using var helper = System.Diagnostics.Process.Start(start) ?? throw new IOException(Localized.Get("无法启动整包更新安装进程。"));
            return 1;
        }
        catch (Exception error)
        {
            Dispatcher.UIThread.Post(() => Abort(Localized.Format("启动安装失败：{0}", error.Message)));
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
    [DllImport(Library, EntryPoint = "win_sparkle_set_app_build_version", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, CharSet = CharSet.Unicode)] private static extern void SetBuildVersion(string version);
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
#endif
