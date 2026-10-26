using System.Text.Json;

namespace CodexMixin;

internal sealed class AppRelease
{
    internal const string ReleasesUrl = "https://github.com/Edward-lyz/codex-mixin/releases/latest";
    internal string Version { get; }
    internal bool IsNewer { get; }
    internal Uri Installer => new($"https://github.com/Edward-lyz/codex-mixin/releases/download/v{Version}/codex-mixin-{Version}-windows-x86_64-setup.exe");
    internal Uri Portable => new($"https://github.com/Edward-lyz/codex-mixin/releases/download/v{Version}/codex-mixin-{Version}-windows-x86_64.zip");

    private AppRelease(string version, bool isNewer) { Version = version; IsNewer = isNewer; }

    internal static AppRelease Parse(JsonElement metadata, string guiVersion)
    {
        static System.Version StableVersion(string value)
        {
            // Stable three-part versions alone can select a downloadable bundle.
            var parts = value.Split('.');
            if (parts.Length != 3 || parts.Any(part => part.Length == 0 || part.Length > 1 && part[0] == '0' || part.Any(character => character is < '0' or > '9'))
                || !System.Version.TryParse(value, out var version))
                throw new CliFailure("自动更新需要有效的稳定版版本号，请从发布页选择完整应用包。");
            return version;
        }
        var latestText = Json.Text(metadata, "latest_version");
        var latest = StableVersion(latestText);
        var cli = StableVersion(Json.Text(metadata, "current_version"));
        var gui = StableVersion(guiVersion);
        if ((cli > latest && gui < latest) || (gui > latest && cli < latest))
            throw new CliFailure("GUI 与 CLI 版本不一致，升级会降级其中一个组件。请从发布页选择完整应用包。");
        return new AppRelease(latestText, latest >= cli && latest >= gui && (latest > cli || latest > gui));
    }
}
