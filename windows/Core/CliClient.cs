using System.Diagnostics;
using System.Text;
using System.Text.Json;

namespace CodexMixin;

internal sealed class CliFailure(string message, bool? committed = null, int? exitCode = null)
    : Exception(message)
{
    internal bool? Committed { get; } = committed;
    internal int? ExitCode { get; } = exitCode;
}

internal sealed class CliClient(string executable)
{
    private const int OutputLimit = 8 * 1024 * 1024;
    private static readonly TimeSpan PipeTimeout = TimeSpan.FromSeconds(3);
    private readonly SemaphoreSlim gate = new(1, 1);
    private readonly SemaphoreSlim queries = new(2, 2);
    // Remote quota work has one separate slot so background prefetch cannot
    // occupy both local snapshot slots (navigation and gateway status).
    private readonly SemaphoreSlim quotaQueries = new(1, 1);
    private readonly SemaphoreSlim updateQueries = new(1, 1);
    private const string QueryWorkerThreads = "2";

    internal static CliClient Locate()
    {
        // Only trust the package or a repository ancestor, never PATH/current directory.
        var directory = new DirectoryInfo(AppContext.BaseDirectory);
        var packaged = Path.Combine(directory.FullName, "codex-mixin.exe");
        if (File.Exists(packaged)) return new(packaged);
        for (var current = directory; current != null; current = current.Parent)
        {
            if (!File.Exists(Path.Combine(current.FullName, "Cargo.toml"))) continue;
            foreach (var configuration in new[] { "release", "debug" })
            {
                var candidate = Path.Combine(current.FullName, "target", configuration, "codex-mixin.exe");
                if (File.Exists(candidate)) return new(candidate);
            }
            break;
        }
        throw new FileNotFoundException("找不到 codex-mixin.exe。请使用完整发布包，或先构建 Rust CLI。");
    }

    internal async Task<JsonElement> QueryAsync(string[] args, CancellationToken cancellation, TimeSpan? timeout = null)
    {
        // Read-only CLI snapshots must not queue behind network-bound actions.
        // Bound subprocesses, and keep process startup/JSON decoding off the UI thread.
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellation);
        deadline.CancelAfter(timeout ?? TimeSpan.FromSeconds(90));
        var family = args.FirstOrDefault() switch
        {
            "service" => "service",
            "provider" => "provider",
            "quota" => "quota",
            "interface" => "interface",
            "update" => "update",
            _ => "other"
        };
        var slots = family switch { "quota" => quotaQueries, "update" => updateQueries, _ => queries };
        var elapsed = Stopwatch.StartNew();
        long? queued = null;
        int? exitCode = null;
        var outcome = "failed";
        var entered = false;
        try
        {
            await slots.WaitAsync(deadline.Token).ConfigureAwait(false);
            entered = true;
            queued = elapsed.ElapsedMilliseconds;
            var snapshot = await Task.Run(async () =>
            {
                var output = await ExecuteAsync(args, deadline.Token, null, null, null, readOnly: true);
                exitCode = 0;
                try
                {
                    using var document = JsonDocument.Parse(output);
                    return document.RootElement.Clone();
                }
                catch (JsonException error)
                {
                    throw new CliFailure($"{args[0]} 返回了无效 JSON：{error.Message}");
                }
            }, deadline.Token).ConfigureAwait(false);
            outcome = "ok";
            return snapshot;
        }
        catch (Exception error) when (deadline.IsCancellationRequested && !cancellation.IsCancellationRequested
            && error is OperationCanceledException or CliFailure)
        {
            outcome = "timeout";
            throw new CliFailure($"{args[0]} 查询超时，请重试。");
        }
        catch (CliFailure error)
        {
            exitCode = error.ExitCode ?? exitCode;
            outcome = cancellation.IsCancellationRequested ? "cancelled" : "failed";
            throw;
        }
        catch (OperationCanceledException)
        {
            outcome = "cancelled";
            throw;
        }
        finally
        {
            if (entered) slots.Release();
            await Diagnostics.RecordAsync("queries", $"{DateTimeOffset.UtcNow:O} query={family} queue_ms={queued ?? elapsed.ElapsedMilliseconds} command_ms={(queued.HasValue ? elapsed.ElapsedMilliseconds - queued.Value : null)} total_ms={elapsed.ElapsedMilliseconds} outcome={outcome} exit_code={exitCode}").ConfigureAwait(false);
        }
    }

    internal async Task<string> RunAsync(string[] args, CancellationToken cancellation,
        IReadOnlyDictionary<string, string>? secrets = null, IProgress<string>? progress = null,
        TimeSpan? timeout = null)
    {
        await gate.WaitAsync(cancellation);
        try { return await Task.Run(() => ExecuteAsync(args, cancellation, secrets, progress, timeout, readOnly: false), cancellation).ConfigureAwait(false); }
        finally { gate.Release(); }
    }

    private async Task<string> ExecuteAsync(string[] args, CancellationToken cancellation,
        IReadOnlyDictionary<string, string>? secrets, IProgress<string>? progress, TimeSpan? timeout, bool readOnly)
    {
        var start = new ProcessStartInfo(executable)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = true,
            StandardOutputEncoding = Encoding.UTF8,
            StandardErrorEncoding = Encoding.UTF8,
            WorkingDirectory = Path.GetDirectoryName(Path.GetFullPath(executable))!
        };
        // Tokio otherwise starts one worker per logical CPU for every short-lived query.
        // Mutation/service commands keep their normal environment and daemon capacity.
        if (readOnly && !start.Environment.ContainsKey("TOKIO_WORKER_THREADS"))
            start.Environment["TOKIO_WORKER_THREADS"] = QueryWorkerThreads;
        start.ArgumentList.Add("--no-tui");
        start.ArgumentList.Add("--json-errors");
        foreach (var argument in args) start.ArgumentList.Add(argument);
        foreach (var (name, value) in secrets ?? new Dictionary<string, string>())
            start.Environment[name] = value;

        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellation);
        deadline.CancelAfter(timeout ?? TimeSpan.FromMinutes(20));
        using var process = new Process { StartInfo = start };
        if (!process.Start()) throw new CliFailure("无法启动 Rust CLI。");
        // GUI callers must never inherit a terminal input handle. DUCX uses
        // this distinction to open its visible QR login window.
        process.StandardInput.Close();
        using var pipes = CancellationTokenSource.CreateLinkedTokenSource(deadline.Token);
        var stdout = ReadAsync(process.StandardOutput, null, secrets, pipes.Token, deadline.Token);
        var stderr = ReadAsync(process.StandardError, progress, secrets, pipes.Token, deadline.Token);
        try
        {
            var exited = process.WaitForExitAsync(deadline.Token);
            var pending = new HashSet<Task> { exited, stdout, stderr };
            while (true)
            {
                var first = await Task.WhenAny(pending);
                await first;
                if (first == exited) break;
                pending.Remove(first);
            }
            // Daemons such as `service start` inherit the pipes and keep them open.
            // The CLI has exited, so its output is complete once this drain window ends.
            pipes.CancelAfter(PipeTimeout);
            var output = await stdout;
            var errors = await stderr;
            if (process.ExitCode != 0) throw DecodeError(errors, output, process.ExitCode);
            return output;
        }
        catch (OperationCanceledException)
        {
            if (!process.HasExited) process.Kill(entireProcessTree: true);
            await process.WaitForExitAsync();
            throw new CliFailure(cancellation.IsCancellationRequested
                ? "操作已取消；已经保存的配置不会自动回滚，请刷新确认状态。"
                : "CLI 超时或输出管道未关闭；配置可能已保存，请刷新确认状态。");
        }
        finally
        {
            pipes.Cancel();
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
                await process.WaitForExitAsync();
            }
            // Observe both readers even when one fails so no tasks outlive the process.
            try { await Task.WhenAll(stdout, stderr); }
            catch (Exception error) when (error is OperationCanceledException or CliFailure or IOException) { }
        }
    }

    private static async Task<string> ReadAsync(StreamReader reader, IProgress<string>? progress,
        IReadOnlyDictionary<string, string>? secrets, CancellationToken drain, CancellationToken deadline)
    {
        var output = new StringBuilder();
        var buffer = new char[4096];
        var pending = new StringBuilder();
        while (true)
        {
            int count;
            try { count = await reader.ReadAsync(buffer.AsMemory(), drain); }
            // Only the post-exit drain window ends a read successfully; timeouts and cancellation still fail.
            catch (OperationCanceledException) when (!deadline.IsCancellationRequested) { break; }
            if (count == 0) break;
            if (output.Length + count > OutputLimit) throw new CliFailure("CLI 输出超过 8 MiB 限制。");
            output.Append(buffer, 0, count);
            if (progress == null) continue;
            for (var index = 0; index < count; index++)
            {
                if (buffer[index] != '\n') { pending.Append(buffer[index]); continue; }
                var line = pending.ToString().TrimEnd('\r');
                pending.Clear();
                if (line.StartsWith("MIXIN_PROGRESS ", StringComparison.Ordinal))
                    progress.Report(Redact(line[15..], secrets));
            }
        }
        return Redact(output.ToString(), secrets);
    }

    private static string Redact(string value, IReadOnlyDictionary<string, string>? secrets)
    {
        foreach (var secret in secrets?.Values ?? [])
        {
            if (string.IsNullOrEmpty(secret)) continue;
            var encoded = JsonSerializer.Serialize(secret)[1..^1];
            value = value.Replace(encoded, "<redacted>", StringComparison.Ordinal);
            if (encoded != secret) value = value.Replace(secret, "<redacted>", StringComparison.Ordinal);
        }
        return value;
    }

    private static CliFailure DecodeError(string errors, string output, int exitCode)
    {
        foreach (var line in errors.Split('\n', StringSplitOptions.RemoveEmptyEntries).Reverse())
        {
            try
            {
                using var document = JsonDocument.Parse(line);
                if (!document.RootElement.TryGetProperty("error", out var error)) continue;
                var committed = error.TryGetProperty("committed", out var flag) &&
                    flag.ValueKind is JsonValueKind.True or JsonValueKind.False ? flag.GetBoolean() : (bool?)null;
                return new CliFailure(Json.Text(error, "message"), committed, exitCode);
            }
            catch (JsonException) { /* Non-JSON diagnostic lines precede the structured error. */ }
        }
        return new CliFailure($"CLI 退出码 {exitCode}：{(string.IsNullOrWhiteSpace(errors) ? output : errors).Trim()}", exitCode: exitCode);
    }
}
