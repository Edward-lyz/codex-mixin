using Microsoft.Win32;

namespace CodexMixin;

// Check complete-package metadata; Windows installation stays with the user.
internal sealed class AppUpdates(Func<CliClient> client)
{
    private const string SettingsKey = @"Software\CodexMixin\Updates";
    private readonly CancellationTokenSource lifetime = new();
    private Task active = Task.CompletedTask;
    internal static string GuiVersion => (typeof(App).Assembly.GetName().Version
        ?? throw new InvalidOperationException("无法读取 GUI 版本。")).ToString(3);
    internal event Action? Changed;
    internal AppRelease? Release { get; private set; }
    internal bool Busy { get; private set; }
    internal string Status { get; private set; } = "尚未检查更新。";
    internal bool Automatic { get; private set; } = true;

    internal void LoadPreferences()
    {
        try
        {
            using var settings = Registry.CurrentUser.OpenSubKey(SettingsKey);
            Automatic = settings?.GetValue("Automatic") is not int enabled || enabled != 0;
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or System.Security.SecurityException)
        {
            Automatic = false;
            Status = $"无法读取更新设置：{error.Message}";
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
        Busy = true;
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
                ? $"发现 {Release.Version}。请下载完整安装包或便携包，完成当前工作后手动安装。"
                : $"当前安装无需升级到发布版 {Release.Version}。";
        }
        catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { Status = "更新检查已取消。"; }
        catch (Exception error)
        {
            Status = $"检查更新失败：{error.Message} 可重试或打开官方发布页。";
            await Diagnostics.RecordAsync("updates", $"{DateTimeOffset.UtcNow:O} stage=metadata outcome=failed error_type={error.GetType().Name}");
        }
        finally { Busy = false; Changed?.Invoke(); }
    }

    internal async Task StopAsync()
    {
        lifetime.Cancel();
        await active;
        lifetime.Dispose();
    }
}
