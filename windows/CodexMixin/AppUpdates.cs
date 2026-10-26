using System.Diagnostics;
using System.Net.Http;
using Microsoft.Win32;

namespace CodexMixin;

// The shell updates the complete Windows package; CLI update replaces only the CLI.
internal sealed class AppUpdates(Func<CliClient> client)
{
    private const string SettingsKey = @"Software\CodexMixin\Updates";
    private const long DownloadLimit = 512L * 1024 * 1024;
    private static readonly HttpClient http = new() { Timeout = TimeSpan.FromMinutes(15) };
    private readonly CancellationTokenSource lifetime = new();
    private Task active = Task.CompletedTask;
    private string? installerPath;
    internal static string GuiVersion => (typeof(App).Assembly.GetName().Version
        ?? throw new InvalidOperationException("无法读取 GUI 版本。")).ToString(3);
    internal event Action? Changed;
    internal AppRelease? Release { get; private set; }
    internal bool Busy { get; private set; }
    internal bool Ready => installerPath != null;
    internal string Status { get; private set; } = "尚未检查更新。";
    internal bool Automatic { get; private set; } = true;
    internal bool Installed { get; private set; }

    internal void LoadPreferences()
    {
        try
        {
            using var settings = Registry.CurrentUser.OpenSubKey(SettingsKey);
            Automatic = settings?.GetValue("Automatic") is not int enabled || enabled != 0;
            foreach (var view in new[] { RegistryView.Registry64, RegistryView.Registry32 })
            {
                using var registry = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, view);
                using var installed = registry.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexMixin");
                if (installed?.GetValue("InstallLocation") is not string directory) continue;
                Installed = Path.TrimEndingDirectorySeparator(Path.GetFullPath(directory)).Equals(
                    Path.TrimEndingDirectorySeparator(Path.GetFullPath(AppContext.BaseDirectory)), StringComparison.OrdinalIgnoreCase);
                if (Installed) break;
            }
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or System.Security.SecurityException or ArgumentException)
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
        var stage = "metadata";
        try
        {
            using var settings = Registry.CurrentUser.CreateSubKey(SettingsKey, true);
            if (automatic && settings.GetValue("NextCheck") is string next
                && DateTimeOffset.TryParse(next, out var nextCheck) && nextCheck > DateTimeOffset.UtcNow) { Status = previousStatus; return; }
            // Persist the backoff before network access so restarting cannot spin.
            settings.SetValue("NextCheck", DateTimeOffset.UtcNow.AddHours(1).ToString("O"));
            var snapshot = await client().QueryAsync(["update", "--check", "--json"], lifetime.Token, TimeSpan.FromSeconds(60));
            var release = AppRelease.Parse(snapshot, GuiVersion);
            if (Release?.Version != release.Version) installerPath = null;
            Release = release;
            if (!release.IsNewer)
            {
                Status = $"当前安装无需升级到发布版 {release.Version}。";
                return;
            }
            Status = $"发现 {release.Version}。";
            if (!Installed) { Status += " 请下载便携整包，退出后解压替换整个应用目录。"; return; }
            if (Ready) { Status += " 安装包已就绪，确认后安装并重启。"; return; }
            stage = "signature";
            await VerifyInstallerAsync(Environment.ProcessPath ?? throw new IOException("无法定位当前应用。"), lifetime.Token);
            var directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMixin", "updates");
            Directory.CreateDirectory(directory);
            var ready = Path.Combine(directory, $"codex-mixin-{release.Version}-setup.exe");
            if (File.Exists(ready))
            {
                try { await VerifyInstallerAsync(ready, lifetime.Token); }
                catch { File.Delete(ready); throw; }
                installerPath = ready;
                Status = $"{release.Version} 完整安装包已就绪，确认后安装并重启。";
                return;
            }
            var target = settings.GetValue("DownloadVersion") as string;
            var attempts = target == release.Version && settings.GetValue("DownloadAttempts") is int count ? count : 0;
            if (automatic && attempts >= 3) { Status += " 自动准备已失败 3 次，请手动重试或打开发布页。"; return; }
            settings.SetValue("DownloadVersion", release.Version);
            settings.SetValue("DownloadAttempts", attempts + 1);
            settings.SetValue("NextCheck", DateTimeOffset.UtcNow.AddHours(Math.Min(4, 1 << Math.Min(attempts, 2))).ToString("O"));
            stage = "download";
            Status = $"正在下载 {release.Version} 完整安装包（安装需要确认）…";
            Changed?.Invoke();
            var partial = ready + ".part.exe";
            using var download = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
            download.CancelAfter(TimeSpan.FromMinutes(15));
            try
            {
                using var response = await http.GetAsync(release.Installer, HttpCompletionOption.ResponseHeadersRead, download.Token);
                response.EnsureSuccessStatusCode();
                if (response.Content.Headers.ContentLength is <= 0 or > DownloadLimit)
                    throw new IOException("安装包大小不符合限制（最大 512 MiB）。");
                await using (var source = await response.Content.ReadAsStreamAsync(download.Token))
                await using (var destination = new FileStream(partial, FileMode.Create, FileAccess.Write, FileShare.None, 64 * 1024, useAsync: true))
                {
                    var buffer = new byte[64 * 1024];
                    long received = 0;
                    int bytes;
                    while ((bytes = await source.ReadAsync(buffer, download.Token)) != 0)
                    {
                        received += bytes;
                        if (received > DownloadLimit) throw new IOException("安装包超过 512 MiB 限制。");
                        await destination.WriteAsync(buffer.AsMemory(0, bytes), download.Token);
                    }
                    if (received == 0 || response.Content.Headers.ContentLength is long expected && received != expected)
                        throw new IOException("安装包下载不完整。");
                }
                stage = "signature";
                await VerifyInstallerAsync(partial, download.Token);
                File.Move(partial, ready, overwrite: true);
                installerPath = ready;
                settings.SetValue("DownloadAttempts", 0);
            }
            finally { if (File.Exists(partial)) File.Delete(partial); }
            Status = $"{release.Version} 安装包已验证。确认后安装并重启（可能显示 UAC）。";
        }
        catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { Status = "更新准备已取消。"; }
        catch (Exception error)
        {
            Status = $"更新失败：{error.Message} 可手动重试或打开官方发布页。";
            await Diagnostics.RecordAsync("updates", $"{DateTimeOffset.UtcNow:O} stage={stage} outcome=failed error_type={error.GetType().Name}");
        }
        finally { Busy = false; Changed?.Invoke(); }
    }

    internal async Task LaunchInstallerAsync(CancellationToken cancellation)
    {
        var path = installerPath ?? throw new InvalidOperationException("请先检查更新并准备完整安装包。");
        await VerifyInstallerAsync(path, cancellation);
        using var installer = Process.Start(new ProcessStartInfo(path) { UseShellExecute = true, Verb = "runas" })
            ?? throw new IOException("无法启动安装器。");
    }

    private static async Task VerifyInstallerAsync(string path, CancellationToken cancellation)
    {
        // Trust Windows signature validation and pin the installed publisher's
        // public key. Unsigned builds/rotated keys require a manual release download.
        const string script = """
            $ErrorActionPreference = 'Stop'
            Import-Module -Name (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
            $installed = Microsoft.PowerShell.Security\Get-AuthenticodeSignature -LiteralPath $env:CODEX_MIXIN_UPDATE_CURRENT
            $candidate = Microsoft.PowerShell.Security\Get-AuthenticodeSignature -LiteralPath $env:CODEX_MIXIN_UPDATE_CANDIDATE
            if ($installed.Status -ne 'Valid') { throw 'Current application has no valid signature; use the official release page' }
            if ($candidate.Status -ne 'Valid') { throw 'Installer signature is invalid' }
            if ($installed.SignerCertificate.GetPublicKeyString() -ne $candidate.SignerCertificate.GetPublicKeyString()) { throw 'Installer publisher does not match the installed application' }
            """;
        var start = new ProcessStartInfo(Path.Combine(Environment.SystemDirectory, "WindowsPowerShell", "v1.0", "powershell.exe"))
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = true
        };
        foreach (var argument in new[] { "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script }) start.ArgumentList.Add(argument);
        start.Environment.Remove("PSModulePath");
        start.Environment["CODEX_MIXIN_UPDATE_CURRENT"] = Environment.ProcessPath ?? throw new IOException("无法定位当前应用。");
        start.Environment["CODEX_MIXIN_UPDATE_CANDIDATE"] = path;
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellation);
        deadline.CancelAfter(TimeSpan.FromSeconds(30));
        using var process = Process.Start(start) ?? throw new IOException("无法启动安装包签名验证。");
        process.StandardInput.Close();
        var output = process.StandardOutput.ReadToEndAsync(deadline.Token);
        var errors = process.StandardError.ReadToEndAsync(deadline.Token);
        try
        {
            await process.WaitForExitAsync(deadline.Token);
            await output;
            if (process.ExitCode != 0) throw new IOException($"安装包签名验证失败：{(await errors).Trim()}");
            await errors;
        }
        finally
        {
            if (!process.HasExited) { process.Kill(entireProcessTree: true); await process.WaitForExitAsync(); }
            try { await Task.WhenAll(output, errors); }
            catch (OperationCanceledException) when (deadline.IsCancellationRequested) { }
        }
    }

    internal async Task StopAsync()
    {
        lifetime.Cancel();
        await active;
        lifetime.Dispose();
    }
}
