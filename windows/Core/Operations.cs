namespace CodexMixin;

internal sealed class Operations(CliClient cli)
{
    internal async Task<string> ChangeAsync(string[] args, CancellationToken cancellation,
        IReadOnlyDictionary<string, string>? secrets = null, IProgress<string>? progress = null,
        bool apply = false)
    {
        var output = await cli.RunAsync(args, cancellation, secrets, progress);
        if (!apply) return output;
        try { await cli.RunAsync(["config", "apply"], cancellation, progress: progress); }
        catch (Exception error) when (error is CliFailure or OperationCanceledException)
        {
            throw new CliFailure($"配置已保存，但应用配置失败：{error.Message}", true, "config apply");
        }
        return output;
    }
}
