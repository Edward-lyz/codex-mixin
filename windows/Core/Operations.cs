using System.Diagnostics;

namespace CodexMixin;

internal sealed class Operations(CliClient cli)
{
    private readonly SemaphoreSlim gate = new(1, 1);
    private long revision;
    internal long Revision => Interlocked.Read(ref revision);
    internal async Task<string> ChangeAsync(string[] args, CancellationToken cancellation,
        IReadOnlyDictionary<string, string>? secrets = null, IProgress<string>? progress = null,
        bool apply = false)
    {
        var elapsed = Stopwatch.StartNew();
        await gate.WaitAsync(cancellation);
        var queued = elapsed.ElapsedMilliseconds;
        long? saved = null;
        var applied = false;
        try
        {
        var output = await cli.RunAsync(args, cancellation, secrets, progress);
        saved = elapsed.ElapsedMilliseconds;
        if (!apply) return output;
        try { await cli.RunAsync(["config", "apply"], cancellation, progress: progress); applied = true; }
        catch (Exception error) when (error is CliFailure or OperationCanceledException)
        {
            throw new CliFailure($"配置已保存，但应用配置失败：{error.Message}", true, "config apply");
        }
        return output;
        }
        finally
        {
            var total = elapsed.ElapsedMilliseconds;
            Interlocked.Increment(ref revision); gate.Release();
            // Only timings and fixed command families; never argv, output or errors.
            var family = args.FirstOrDefault() is "provider" or "config" or "agent" or "context" or "service"
                ? args[0] : "other";
            await RecordTimingAsync($"{DateTimeOffset.UtcNow:O} action={family} queue_ms={queued} command_ms={saved - queued} apply_ms={(saved.HasValue && apply ? total - saved.Value : null)} total_ms={total} command_completed={saved.HasValue} apply_completed={applied}");
        }
    }

    private static readonly SemaphoreSlim logGate = new(1, 1);
    private static async Task RecordTimingAsync(string line)
    {
        await logGate.WaitAsync();
        try
        {
            await Task.Run(() =>
            {
                var directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMixin", "logs");
                Directory.CreateDirectory(directory);
                var path = Path.Combine(directory, "actions.log");
                if (File.Exists(path) && new FileInfo(path).Length > 1024 * 1024)
                    File.Move(path, path + ".previous", overwrite: true);
                File.AppendAllText(path, line + Environment.NewLine);
            });
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
            Trace.TraceWarning("Cannot write action timing log: {0}", error.GetType().Name);
        }
        finally { logGate.Release(); }
    }
}
