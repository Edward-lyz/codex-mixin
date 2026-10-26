using System.Diagnostics;
using System.Text.Json;
using CodexMixin;

if (args.Contains("--fixture"))
{
    var flags = args.SkipWhile(value => value != "--json-errors").Skip(1).ToArray();
    switch (flags.FirstOrDefault())
    {
        case "echo":
            Console.Write(JsonSerializer.Serialize(new { args = flags, secret = Environment.GetEnvironmentVariable("CM_TEST_SECRET"), worker_threads = Environment.GetEnvironmentVariable("TOKIO_WORKER_THREADS") }));
            break;
        case "stdin":
            Console.Write(JsonSerializer.Serialize(new { redirected = Console.IsInputRedirected, eof = Console.In.ReadToEnd() == "" }));
            break;
        case "update":
            File.WriteAllText(flags[1], "started");
            await Task.Delay(TimeSpan.FromSeconds(120));
            break;
        case "bad-json": Console.Write("not JSON"); break;
        case "error":
            Console.Error.WriteLine("diagnostic line");
            Console.Error.WriteLine("{\"error\":{\"message\":\"saved but sync failed\",\"committed\":true,\"stage\":\"client sync\"}}");
            return 7;
        case "save": Console.WriteLine("saved"); break;
        case "config": Console.Error.WriteLine("apply failed"); return 8;
        case "quota":
            File.WriteAllText(flags[1], "started");
            await Task.Delay(TimeSpan.FromSeconds(120));
            break;
        case "sleep": await Task.Delay(TimeSpan.FromSeconds(120)); break;
        case "slow-query":
            File.WriteAllText(flags[1], "started");
            await Task.Delay(TimeSpan.FromSeconds(30));
            Console.Write("{}");
            break;
        case "progress":
            Console.Error.WriteLine("MIXIN_PROGRESS step one");
            Console.Error.WriteLine("MIXIN_PROGRESS " + Environment.GetEnvironmentVariable("CM_TEST_SECRET"));
            Console.Write("done"); break;
        case "lock":
            using (var locked = new FileStream(flags[1], FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.None))
            {
                await Task.Delay(150);
                Console.Write(flags[2]);
            }
            break;
        case "flood":
            for (var index = 0; index < 2500; index++) Console.Write(new string('x', 4096));
            await Task.Delay(TimeSpan.FromSeconds(120)); break;
        case "daemon":
            // Like `service start`: a detached child keeps the inherited output pipes open.
            Process.Start(new ProcessStartInfo(Environment.ProcessPath!,
                [typeof(CliClient).Assembly.Location, "--fixture", "--json-errors", "hold"])
            { UseShellExecute = false });
            Console.Write("{\"started\":true}");
            break;
        case "hold": await Task.Delay(TimeSpan.FromSeconds(15)); break;
        default: throw new InvalidOperationException("Unknown fixture command");
    }
    return 0;
}

var dotnet = Environment.GetEnvironmentVariable("DOTNET_HOST_PATH") ?? "dotnet";
var assembly = typeof(CliClient).Assembly.Location;
var fixture = new CliClient(dotnet, [assembly, "--fixture"]);
var cliOnly = args.Contains("--cli-only");
if (cliOnly && args is not ["--cli-only", _])
    throw new ArgumentException("--cli-only requires a CLI path");
var checks = new List<(string Name, Func<Task> Check)>();
void AddCheck(string name, Func<Task> check)
{
    if (checks.Any(existing => existing.Name == name))
        throw new InvalidOperationException($"Duplicate check: {name}");
    checks.Add((name, check));
}
void Check(string name, Func<Task> check) { if (!cliOnly) AddCheck(name, check); }
void Assert(bool condition, string message) { if (!condition) throw new InvalidOperationException(message); }
JsonElement Parse(string value) { using var document = JsonDocument.Parse(value); return document.RootElement.Clone(); }
async Task<CliFailure> Failure(Func<Task> call)
{
    try { await call(); }
    catch (CliFailure error) { return error; }
    throw new InvalidOperationException("Expected an explicit failure");
}

Check("quota display cache survives restart without persisting credentials or raw data", async () =>
{
    var path = Path.Combine(Path.GetTempPath(), $"mixin-display-{Guid.NewGuid():N}", "quota.json");
    var updated = DateTimeOffset.UtcNow;
    try
    {
        var cache = QuotaDisplayCache.Create(
            Json.Rows(Parse("""[{"id":"a","display_name":"A","icon":"custom","api_key":"fixture-secret"}]""")),
            Json.Rows(Parse("""[{"provider_id":"a","quota_id":"weekly","used":51,"limit":100,"raw":{"token":"fixture-secret"}},{"provider_id":"removed","used":90},{"provider_id":"a","error":"fixture-error"}]""")),
            new() { ["a"] = updated, ["removed"] = updated });
        await cache.SaveAsync(path, CancellationToken.None);
        var raw = await File.ReadAllTextAsync(path);
        Assert(!raw.Contains("fixture-secret", StringComparison.Ordinal) && !raw.Contains("fixture-error", StringComparison.Ordinal), "Cache persisted non-display data");
        var loaded = await QuotaDisplayCache.LoadAsync(path, CancellationToken.None);
        Assert(loaded is { Providers.Length: 1, Rows.Length: 1 } && Json.Real(loaded.Rows[0], "used") == 51, "Measured quota did not survive restart");
        Assert(loaded!.UpdatedAt.Count == 1 && loaded.UpdatedAt["a"] == updated, "Cache lost measured time or retained removed provider");
        await File.WriteAllTextAsync(path, "{}");
        try { await QuotaDisplayCache.LoadAsync(path, CancellationToken.None); }
        catch (InvalidDataException) { return; }
        throw new InvalidOperationException("Invalid cache was accepted");
    }
    finally { if (Directory.Exists(Path.GetDirectoryName(path))) Directory.Delete(Path.GetDirectoryName(path)!, true); }
});
Check("background quota leaves both local query slots available", async () =>
{
    var marker = Path.Combine(Path.GetTempPath(), $"mixin-quota-{Guid.NewGuid():N}");
    var localMarker = marker + "-local";
    using var cancellation = new CancellationTokenSource();
    var quota = fixture.QueryAsync(["quota", marker], cancellation.Token);
    var local = fixture.QueryAsync(["slow-query", localMarker], cancellation.Token);
    try
    {
        var started = Stopwatch.StartNew();
        while ((!File.Exists(marker) || !File.Exists(localMarker)) && started.Elapsed < TimeSpan.FromSeconds(10)) await Task.Delay(20);
        Assert(File.Exists(marker) && File.Exists(localMarker), "Remote and local reads did not start");
        await fixture.QueryAsync(["echo"], CancellationToken.None, TimeSpan.FromSeconds(5));
        Assert(!quota.IsCompleted && !local.IsCompleted, "Local query interrupted unrelated reads");
    }
    finally
    {
        cancellation.Cancel();
        await Failure(() => quota);
        await Failure(() => local);
        File.Delete(marker); File.Delete(localMarker);
    }
});
Check("slow quota query does not block a local snapshot", async () =>
{
    var marker = Path.Combine(Path.GetTempPath(), $"mixin-query-{Guid.NewGuid():N}");
    using var cancellation = new CancellationTokenSource();
    var slow = fixture.QueryAsync(["slow-query", marker], cancellation.Token);
    try
    {
        var started = Stopwatch.StartNew();
        while (!File.Exists(marker) && started.Elapsed < TimeSpan.FromSeconds(10)) await Task.Delay(20);
        Assert(File.Exists(marker), "Slow query did not start");
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        var local = await fixture.QueryAsync(["echo"], deadline.Token);
        Assert(Json.Strings(Json.Field(local, "args")).SequenceEqual(new[] { "echo" }), "Local query was blocked by quota");
    }
    finally
    {
        cancellation.Cancel();
        await Failure(() => slow);
        File.Delete(marker);
    }
});
Check("query deadline includes waiting for a subprocess slot", async () =>
{
    var markers = Enumerable.Range(0, 2).Select(_ => Path.Combine(Path.GetTempPath(), $"mixin-deadline-{Guid.NewGuid():N}")).ToArray();
    using var cancellation = new CancellationTokenSource();
    var pending = markers.Select(marker => fixture.QueryAsync(["slow-query", marker], cancellation.Token)).ToArray();
    try
    {
        var started = Stopwatch.StartNew();
        while (!markers.All(File.Exists) && started.Elapsed < TimeSpan.FromSeconds(10)) await Task.Delay(20);
        Assert(markers.All(File.Exists), "Both query slots must be occupied");
        var elapsed = Stopwatch.StartNew();
        var error = await Failure(() => fixture.QueryAsync(["echo"], CancellationToken.None, TimeSpan.FromMilliseconds(300)));
        Assert(error.Message.Contains("超时", StringComparison.Ordinal), "Queue timeout was not reported");
        Assert(elapsed.Elapsed < TimeSpan.FromSeconds(5), "Queue wait ignored the query deadline");
        Assert(pending.All(task => !task.IsCompleted), "Queued timeout cancelled unrelated queries");
    }
    finally
    {
        cancellation.Cancel();
        foreach (var task in pending) await Failure(() => task);
        foreach (var marker in markers) File.Delete(marker);
    }
    await fixture.QueryAsync(["echo"], CancellationToken.None);
});
Check("query deadline terminates a running child and releases its slot", async () =>
{
    var marker = Path.Combine(Path.GetTempPath(), $"mixin-running-deadline-{Guid.NewGuid():N}");
    try
    {
        var error = await Failure(() => fixture.QueryAsync(["slow-query", marker], CancellationToken.None, TimeSpan.FromSeconds(3)));
        Assert(File.Exists(marker), "Timed query never started its child");
        Assert(error.Message.Contains("超时", StringComparison.Ordinal), "Running timeout was not reported");
        await fixture.QueryAsync(["echo"], CancellationToken.None);
    }
    finally { File.Delete(marker); }
});
Check("GUI commands receive redirected stdin at EOF", async () =>
{
    var stdin = await fixture.QueryAsync(["stdin"], CancellationToken.None);
    Assert(Json.Flag(stdin, "redirected") && Json.Flag(stdin, "eof"), "GUI CLI inherited terminal input instead of using the visible-login path");
});
Check("quota and update requests leave a local query slot available", async () =>
{
    var markers = Enumerable.Range(0, 3).Select(_ => Path.Combine(Path.GetTempPath(), $"mixin-independent-{Guid.NewGuid():N}")).ToArray();
    using var cancellation = new CancellationTokenSource();
    var pending = new[]
    {
        fixture.QueryAsync(["quota", markers[0]], cancellation.Token),
        fixture.QueryAsync(["update", markers[1]], cancellation.Token),
        fixture.QueryAsync(["slow-query", markers[2]], cancellation.Token)
    };
    try
    {
        var started = Stopwatch.StartNew();
        while (!markers.All(File.Exists) && started.Elapsed < TimeSpan.FromSeconds(10)) await Task.Delay(20);
        Assert(markers.All(File.Exists), "Independent remote/local requests did not start");
        var local = await fixture.QueryAsync(["echo"], CancellationToken.None, TimeSpan.FromSeconds(5));
        Assert(Json.Strings(Json.Field(local, "args")).SequenceEqual(new[] { "echo" }), "Remote update/quota work blocked the local snapshot");
    }
    finally
    {
        cancellation.Cancel();
        foreach (var task in pending) await Failure(() => task);
        foreach (var marker in markers) File.Delete(marker);
    }
});
Check("Windows updates compare GUI and CLI versions before selecting a complete bundle", async () =>
{
    AppRelease Release(string current, string latest, string gui) => AppRelease.Parse(Parse(JsonSerializer.Serialize(
        new { current_version = current, latest_version = latest })), gui);
    var newer = Release("0.9.9", "0.10.0", "0.9.9");
    Assert(newer.IsNewer && newer.Installer.AbsoluteUri.EndsWith("/v0.10.0/codex-mixin-0.10.0-windows-x86_64-setup.exe", StringComparison.Ordinal), "Update did not select the complete installer");
    Assert(newer.Portable.AbsoluteUri.EndsWith("/codex-mixin-0.10.0-windows-x86_64.zip", StringComparison.Ordinal), "Portable update selected a CLI-only archive");
    Assert(!Release("1.0.0", "1.0.0", "1.0.0").IsNewer, "Equal version triggered an update");
    Assert(!Release("1.1.0", "1.0.0", "1.1.0").IsNewer, "Update would downgrade the current bundle");
    var cliRepair = Release("1.0.0", "1.0.1", "1.0.1");
    Assert(cliRepair.IsNewer && cliRepair.CurrentVersion == "1.0.0", "WinSparkle would skip repair of an older bundled CLI");
    Assert(Release("1.0.1", "1.0.1", "1.0.0").IsNewer, "Older GUI was not repaired");
    await Failure(() => Task.FromResult(Release("1.0.0", "1.0.1", "1.1.0")));
    foreach (var invalid in new[] { "", "1.2", "1.2.3.4", "1.2.3-beta", "v1.2.3", "../1.2.3", "01.2.3", "999999999999.0.0" })
        await Failure(() => Task.FromResult(Release("1.0.0", invalid, "1.0.0")));
});
Check("usage buckets preserve chart values with one scan", () =>
{
    var rows = Json.Rows(Parse("[{\"provider_id\":\"a\",\"day\":9,\"value\":3,\"tokens\":4},{\"provider_id\":\"b\",\"day\":10,\"value\":5,\"tokens\":9},{\"provider_id\":\"a\",\"day\":10,\"value\":7,\"tokens\":6},{\"provider_id\":\"b\",\"day\":10,\"value\":2,\"tokens\":1},{\"provider_id\":\"c\",\"day\":1,\"value\":99,\"tokens\":100}]"));
    foreach (var split in new[] { false, true })
    {
        var metricCalls = 0;
        var buckets = UsageBuckets.Build(rows, 2, split, 10, row => { metricCalls++; return Json.Field(row, "value").GetInt64(); }, row => Json.Field(row, "tokens").GetInt64());
        var series = rows.GroupBy(row => split ? Json.Text(row, "provider_id") : "全部")
            .OrderByDescending(group => group.Sum(row => Json.Field(row, "tokens").GetInt64())).Select(group => group.Key).ToArray();
        var expected = new long[] { 9, 10 }.Select(day => series.Select(key => rows
            .Where(row => Json.Field(row, "day").GetInt64() == day && (split ? Json.Text(row, "provider_id") : "全部") == key)
            .Sum(row => Json.Field(row, "value").GetInt64())).ToArray()).ToArray();
        Assert(buckets.Series.SequenceEqual(series), "Chart series/tie order changed");
        Assert(buckets.Days.SequenceEqual(new long[] { 9, 10 }), "Chart dates changed");
        Assert(buckets.Values.Zip(expected).All(pair => pair.First.SequenceEqual(pair.Second)), "Chart cells changed");
        Assert(buckets.Total == 116 && buckets.Maximum == 14, "Chart totals or scale changed");
        Assert(metricCalls == rows.Length, "Chart repeatedly scanned the CLI rows");
    }
    var empty = UsageBuckets.Build([], 7, true, 10, _ => 0, _ => 0);
    Assert(empty.Maximum == 1 && empty.Total == 0 && empty.Values.Length == 7, "Empty chart changed");
    try
    {
        UsageBuckets.Build(rows.Take(2).ToArray(), 7, false, 10, _ => long.MaxValue, _ => 0);
        throw new InvalidOperationException("Chart counter overflow was silently accepted");
    }
    catch (OverflowException) { }
    return Task.CompletedTask;
});
Check("mutation attempts invalidate presentation snapshots", async () =>
{
    var operations = new Operations(fixture);
    var revision = operations.Revision;
    await operations.ChangeAsync(["save"], CancellationToken.None);
    Assert(operations.Revision == revision + 1, "Successful mutation kept an old snapshot revision");
    await Failure(() => operations.ChangeAsync(["error"], CancellationToken.None));
    Assert(operations.Revision == revision + 2, "Failed/partially committed mutation kept an old snapshot revision");
});
Check("query worker budget does not alter mutation or parent environment", async () =>
{
    var inherited = Environment.GetEnvironmentVariable("TOKIO_WORKER_THREADS");
    var query = await fixture.QueryAsync(["echo"], CancellationToken.None);
    Assert(Json.OptionalText(query, "worker_threads") == (inherited ?? "2"), "Query worker budget is missing");
    var mutation = Parse(await fixture.RunAsync(["echo"], CancellationToken.None));
    Assert(Json.OptionalText(mutation, "worker_threads") == inherited, "Mutation/daemon worker capacity changed");
    Assert(Environment.GetEnvironmentVariable("TOKIO_WORKER_THREADS") == inherited, "Worker budget leaked into the parent environment");
});
Check("argument boundaries and shell metacharacters", async () =>
{
    var values = new[] { "echo", "value with spaces", "quote\"value", "$(whoami)", "& exit 9", "中文参数" };
    var output = await fixture.QueryAsync(values, CancellationToken.None);
    Assert(Json.Strings(Json.Field(output, "args")).SequenceEqual(values), "Arguments were changed");
});
Check("secret environment and redacted output", async () =>
{
    const string secret = "unique-test-secret-\"\\\n";
    var output = await fixture.RunAsync(["echo", "--key", "@env:CM_TEST_SECRET"], CancellationToken.None,
        new Dictionary<string, string> { ["CM_TEST_SECRET"] = secret });
    var row = Parse(output);
    Assert(Json.Text(row, "secret") == "<redacted>", "Environment value was not redacted");
    Assert(!output.Contains(secret), "Credential leaked into output");
    Assert(Json.Strings(Json.Field(row, "args")).Contains("@env:CM_TEST_SECRET"), "Secret reference was lost");
    Assert(Environment.GetEnvironmentVariable("CM_TEST_SECRET") == null, "Secret changed the parent environment");
});
Check("invalid JSON is an error", async () =>
    await Failure(() => fixture.QueryAsync(["bad-json"], CancellationToken.None)));
Check("structured errors preserve commit and stage", async () =>
{
    var error = await Failure(() => fixture.RunAsync(["error"], CancellationToken.None));
    Assert(error.Committed == true && error.Stage == "client sync", "Commit state was lost");
    Assert(error.Message == "saved but sync failed", "Structured message was lost");
    Assert(error.ExitCode == 7, "Actual CLI exit code was lost");
});
Check("apply failure is reported after commit", async () =>
{
    var error = await Failure(() => new Operations(fixture).ChangeAsync(["save"], CancellationToken.None, apply: true));
    Assert(error.Committed == true && error.Stage == "config apply", "Apply failure claimed rollback");
    Assert(error.ExitCode == 8, "Apply failure lost its actual exit code");
});
Check("timeout kills the process and frees the queue", async () =>
{
    await Failure(() => fixture.RunAsync(["sleep"], CancellationToken.None, timeout: TimeSpan.FromMilliseconds(300)));
    Assert(!string.IsNullOrWhiteSpace(await fixture.RunAsync(["echo"], CancellationToken.None)), "Queue remained blocked");
});
Check("cancellation terminates an action", async () =>
{
    using var cancellation = new CancellationTokenSource(TimeSpan.FromMilliseconds(300));
    var error = await Failure(() => fixture.RunAsync(["sleep"], cancellation.Token));
    Assert(error.Message.Contains("取消"), "Cancellation was reported as success");
});
Check("progress is delivered and redacted", async () =>
{
    var steps = new List<string>();
    await fixture.RunAsync(["progress"], CancellationToken.None,
        new Dictionary<string, string> { ["CM_TEST_SECRET"] = "private-progress" }, new InlineProgress(steps));
    Assert(steps.SequenceEqual(new[] { "step one", "<redacted>" }), "Progress was lost or leaked a secret");
});
Check("bounded output fails and releases the queue", async () =>
{
    var error = await Failure(() => fixture.RunAsync(["flood"], CancellationToken.None, timeout: TimeSpan.FromSeconds(5)));
    Assert(error.Message.Contains("8 MiB"), "Output limit did not fail immediately");
    await fixture.RunAsync(["echo"], CancellationToken.None);
});
Check("successful exit keeps output when a daemon holds the pipes", async () =>
{
    var output = await fixture.QueryAsync(["daemon"], CancellationToken.None);
    Assert(Json.Flag(output, "started"), "Daemon start output was lost");
});
Check("concurrent mutations are serialized", async () =>
{
    var path = Path.Combine(Path.GetTempPath(), "mixin-queue-" + Guid.NewGuid().ToString("N"));
    try
    {
        var queries = Enumerable.Range(0, 6).Select(index => fixture.RunAsync(["lock", path, index.ToString()], CancellationToken.None));
        var results = await Task.WhenAll(queries);
        Assert(results.Order().SequenceEqual(new[] { "0", "1", "2", "3", "4", "5" }), "Request outputs crossed");
    }
    finally { File.Delete(path); }
});
Check("fresh installation is unconfigured", () =>
{
    Assert(Json.Gateway(Parse("{\"configured\":false}")) == "未配置", "Fresh install was labeled stopped");
    return Task.CompletedTask;
});
Check("unknown or malformed state does not imply stopped", async () =>
{
    await Failure(() => { Json.Gateway(Parse("{\"configured\":true,\"gateway\":\"unknown\"}")); return Task.CompletedTask; });
    await Failure(() => { Json.Gateway(Parse("{\"configured\":true}")); return Task.CompletedTask; });
    await Failure(() => { Json.Flag(Parse("{\"enabled\":\"true\"}"), "enabled"); return Task.CompletedTask; });
});
Check("contract arrays reject incomplete data", async () =>
{
    await Failure(() => { Json.Rows(Parse("[{},null]")); return Task.CompletedTask; });
    await Failure(() => { Json.Strings(Parse("[\"model\",123]")); return Task.CompletedTask; });
});

Check("context contract rejects unknown values", async () =>
{
    var missing = Parse("""
        {"context":{},"config_path":"/tmp/config.json","database_path":"/tmp/context.db"}
        """);
    await Failure(() => { ContextContract.ParseConfig(missing); return Task.CompletedTask; });
    var outOfRange = Parse("""
        {"enabled":true,"database_path":"/tmp/context.db","totals":{"sessions":0,"messages":0,"compressions":0,"saved_tokens":0,"view_tokens":0,"cache":{"turns":0,"input_tokens":0,"cached_tokens":0,"output_tokens":0,"hit_percent":101}},"sessions":[]}
        """);
    await Failure(() => { ContextContract.ParseStats(outOfRange); return Task.CompletedTask; });
    var unavailable = Parse("""
        {"enabled":true,"database_path":"/tmp/context.db","totals":{"sessions":0,"messages":0,"compressions":0,"saved_tokens":0,"view_tokens":0,"cache":{"turns":1,"input_tokens":32000,"cached_tokens":0,"output_tokens":100,"hit_percent":null}},"sessions":[]}
        """);
    var cache = ContextContract.ParseStats(unavailable).Totals.Cache;
    Assert(cache.HitPercent is null && cache.InputTokens == 32000,
        "Unavailable cache metrics must retain billed tokens");
    var valid = Parse("""
        {"context":{"enabled":true,"growth_threshold_tokens":80000,"pressure_ratio":0.6,"urgent_ratio":0.8,"protected_messages":8,"protected_tokens":24000,"max_internal_rounds":2,"max_tier":2,"guidance_only":false,"context_window_tokens":128000},"config_path":"/tmp/config.json","database_path":"/tmp/context.db"}
        """);
    var config = ContextContract.ParseConfig(valid);
    Assert(config.Settings.GrowthThresholdTokens == 80000 && config.Settings.ContextWindowTokens == 128000,
        "Valid context settings were not parsed");
});
Check("context save sends only changed fields", () =>
{
    var stored = new ContextSettings(true, 80000, 0.6, 0.8, 8, 24000, 2, 2, false, 128000);
    Assert(ContextContract.SaveArguments(stored, stored) is null, "Unchanged settings must not call the CLI");
    var edited = stored with { Enabled = false, ContextWindowTokens = null };
    var arguments = ContextContract.SaveArguments(stored, edited);
    Assert(arguments is not null && arguments.SequenceEqual(["context", "config", "set", "--enabled", "false", "--context-window-tokens", "none"]),
        "A save must send exactly the changed flags");
    return Task.CompletedTask;
});

string? realCli = null;
if (args is ["--cli", var cliArgument]) realCli = cliArgument;
else if (args is ["--cli-only", var cliOnlyArgument]) realCli = cliOnlyArgument;
if (realCli is not null)
{
    var cliPath = Path.GetFullPath(realCli);
    AddCheck("real Rust CLI interface and read-only contracts", async () =>
    {
        var directory = Path.Combine(Path.GetTempPath(), "mixin-contract-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(directory);
        var oldConfig = Environment.GetEnvironmentVariable("CODEX_GATEWAY_CONFIG");
        var oldCodexHome = Environment.GetEnvironmentVariable("CODEX_HOME");
        try
        {
            Environment.SetEnvironmentVariable("CODEX_GATEWAY_CONFIG", Path.Combine(directory, "config.json"));
            // No ChatGPT OAuth session may leak in from the runner's real Codex home.
            Environment.SetEnvironmentVariable("CODEX_HOME", Path.Combine(directory, "codex"));
            var client = new CliClient(cliPath);
            var metadata = await client.QueryAsync(["interface", "--json"], CancellationToken.None);
            Json.Interface(metadata);
            var catalog = Json.Field(metadata, "provider_catalog");
            var presets = Json.Rows(Json.Field(catalog, "presets"));
            var agy = presets.Single(row => Json.Text(row, "id") == "agy");
            Assert(Json.Text(agy, "auth_mode") == "local_oauth" && !Json.Flag(agy, "api_key_required"),
                "Antigravity must support local OAuth without an API key");
            foreach (var id in new[] { "kiro", "agy-business" })
            {
                var preset = presets.Single(row => Json.Text(row, "id") == id);
                Assert(Json.Text(preset, "auth_mode") == "local_session" && !Json.Flag(preset, "api_key_required"),
                    $"{id} must support a local session without an API key");
            }
            var invalidAuth = System.Text.Json.Nodes.JsonNode.Parse(metadata.GetRawText())!;
            invalidAuth["provider_catalog"]!["presets"]![0]!["auth_mode"] = "unsupported";
            await Failure(() => { Json.Interface(JsonSerializer.SerializeToElement(invalidAuth)); return Task.CompletedTask; });
            Assert(Json.Rows(Json.Field(catalog, "protocols")).Any(row => Json.Text(row, "id") == "google_gemini"),
                "The CLI must expose the Gemini protocol to the Windows picker");
            var status = await client.QueryAsync(["service", "status", "--json"], CancellationToken.None);
            Assert(Json.Gateway(status) == "未配置", "Fresh CLI unexpectedly reported a configured gateway");
            // `provider list` is a read-only query: an unconfigured install lists no providers.
            Assert(Json.Rows(Json.Field(await client.QueryAsync(["provider", "list", "--json"], CancellationToken.None), "providers")).Length == 0,
                "A fresh install must list no providers");
            Json.Rows(Json.Field(await client.QueryAsync(["agent", "list", "--json"], CancellationToken.None), "agents"));
            Json.Flag(await client.QueryAsync(["service", "autostart", "status", "--json"], CancellationToken.None), "enabled");
            Json.Flag(await client.QueryAsync(["ech", "status", "--json"], CancellationToken.None), "enabled");
            Json.Field(await client.QueryAsync(["fusion", "get", "--json"], CancellationToken.None), "profile");
            var contextConfig = ContextContract.ParseConfig(await client.QueryAsync(["context", "config", "get", "--json"], CancellationToken.None));
            Assert(!contextConfig.Settings.Enabled, "A fresh install must keep the context engine disabled");
            var contextStats = ContextContract.ParseStats(await client.QueryAsync(["context", "stats", "--json"], CancellationToken.None));
            Assert(contextStats.Totals.Sessions == 0 && contextStats.Sessions.Length == 0,
                "A fresh install must report no archived context sessions");
        }
        finally
        {
            Environment.SetEnvironmentVariable("CODEX_GATEWAY_CONFIG", oldConfig);
            Environment.SetEnvironmentVariable("CODEX_HOME", oldCodexHome);
            Directory.Delete(directory, true);
        }
    });
}

var failures = 0;
foreach (var (name, check) in checks)
{
    try { await check(); Console.WriteLine($"PASS {name}"); }
    catch (Exception error) { failures++; Console.Error.WriteLine($"FAIL {name}: {error}"); }
}
Console.WriteLine($"{checks.Count - failures}/{checks.Count} checks passed");
return failures == 0 ? 0 : 1;

internal sealed class InlineProgress(List<string> steps) : IProgress<string>
{
    public void Report(string value) => steps.Add(value);
}
