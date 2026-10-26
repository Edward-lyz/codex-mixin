using Microsoft.Win32;

namespace CodexMixin;

// The GUI owns idle scheduling; WinSparkle owns signed complete-package updates.
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
        ?? throw new InvalidOperationException("无法读取 GUI 版本。")).ToString(3);
    internal event Action? Changed;
    internal AppRelease? Release { get; private set; }
    internal bool Busy => checking || updating;
    internal string Status { get; private set; } = "尚未检查更新。";
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
                Status = $"上次更新失败：{error}";
            }
            updater = new NativeUpdater(this);
            updater.Initialize();
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or System.Security.SecurityException or DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
        {
            Automatic = false;
            Status = $"更新器初始化失败：{error.Message}";
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
        Status = "正在检查 Windows 完整应用更新…";
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
                ? $"发现 {Release.Version}。"
                : $"当前安装无需升级到发布版 {Release.Version}。";
            if (!Release.IsNewer) return;
            automaticInstall = automatic;
            if (!(await PrepareInstallAsync(lifetime.Token)).Allowed) return;
            var attempts = settings.GetValue("AttemptVersion") as string == Release.Version && settings.GetValue("Attempts") is int count ? count : 0;
            if (automatic && attempts >= 3) { Status = "自动更新已重试三次，请点立即更新重试。"; return; }
            settings.SetValue("AttemptVersion", Release.Version);
            settings.SetValue("Attempts", attempts + 1);
            settings.SetValue("NextCheck", DateTimeOffset.UtcNow.AddHours(Math.Min(4, 1 << Math.Min(attempts, 2))).ToString("O"));
            updater ??= new NativeUpdater(this);
            updating = true;
            Status = $"正在更新到 {Release.Version}：下载和验签由 WinSparkle 处理，完成后自动安装并重启。";
            updater.Start();
        }
        catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { Status = "更新检查已取消。"; }
        catch (Exception error)
        {
            updating = false;
            Status = $"更新失败：{error.Message} 可点立即更新重试。";
            await Diagnostics.RecordAsync("updates", $"{DateTimeOffset.UtcNow:O} stage=metadata outcome=failed error_type={error.GetType().Name}");
        }
        finally { checking = false; Changed?.Invoke(); }
    }

    internal async Task<(bool Allowed, bool Running)> PrepareInstallAsync(CancellationToken token)
    {
        var app = (App)Microsoft.UI.Xaml.Application.Current;
        if (app.HasPendingChanges || app.HasDraftChanges)
        {
            Status = "暂缓更新：请先保存草稿并完成当前操作。";
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
                Status = automaticInstall ? "等待网关空闲 30 分钟后自动更新。" : "正在处理请求，完成后请再点立即更新。";
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
