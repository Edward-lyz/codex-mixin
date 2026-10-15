using System.Diagnostics;
using System.Text.Json;
using CodexMixin;

if (args.Contains("--fixture"))
{
    var flags = args.SkipWhile(value => value != "--json-errors").Skip(1).ToArray();
    switch (flags.FirstOrDefault())
    {
        case "echo":
            Console.Write(JsonSerializer.Serialize(new { args = flags, secret = Environment.GetEnvironmentVariable("CM_TEST_SECRET") }));
            break;
        case "bad-json": Console.Write("not JSON"); break;
        case "error":
            Console.Error.WriteLine("diagnostic line");
            Console.Error.WriteLine("{\"error\":{\"message\":\"saved but sync failed\",\"committed\":true,\"stage\":\"client sync\"}}");
            return 7;
        case "save": Console.WriteLine("saved"); break;
        case "config": Console.Error.WriteLine("apply failed"); return 8;
        case "sleep": await Task.Delay(TimeSpan.FromSeconds(120)); break;
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
                [typeof(CliClient).Assembly.Location, "--fixture", "--json-errors", "hold"]) { UseShellExecute = false });
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
var checks = new List<(string Name, Func<Task> Check)>();
void Check(string name, Func<Task> check) => checks.Add((name, check));
void Assert(bool condition, string message) { if (!condition) throw new InvalidOperationException(message); }
JsonElement Parse(string value) { using var document = JsonDocument.Parse(value); return document.RootElement.Clone(); }
async Task<CliFailure> Failure(Func<Task> call)
{
    try { await call(); }
    catch (CliFailure error) { return error; }
    throw new InvalidOperationException("Expected an explicit failure");
}

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
});
Check("apply failure is reported after commit", async () =>
{
    var error = await Failure(() => new Operations(fixture).ChangeAsync(["save"], CancellationToken.None, apply: true));
    Assert(error.Committed == true && error.Stage == "config apply", "Apply failure claimed rollback");
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

if (args is ["--cli", var realCli])
{
    Check("real Rust CLI interface and read-only contracts", async () =>
    {
        var directory = Path.Combine(Path.GetTempPath(), "mixin-contract-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(directory);
        var oldConfig = Environment.GetEnvironmentVariable("CODEX_GATEWAY_CONFIG");
        try
        {
            Environment.SetEnvironmentVariable("CODEX_GATEWAY_CONFIG", Path.Combine(directory, "config.json"));
            var client = new CliClient(Path.GetFullPath(realCli));
            var metadata = await client.QueryAsync(["interface", "--json"], CancellationToken.None);
            Json.Interface(metadata);
            var catalog = Json.Field(metadata, "provider_catalog");
            var presets = Json.Rows(Json.Field(catalog, "presets"));
            var agy = presets.Single(row => Json.Text(row, "id") == "agy");
            Assert(Json.Text(agy, "auth_mode") == "local_oauth" && !Json.Flag(agy, "api_key_required"),
                "Antigravity must support local OAuth without an API key");
            Assert(Json.Rows(Json.Field(catalog, "protocols")).Any(row => Json.Text(row, "id") == "google_gemini"),
                "The CLI must expose the Gemini protocol to the Windows picker");
            var status = await client.QueryAsync(["service", "status", "--json"], CancellationToken.None);
            Assert(Json.Gateway(status) == "未配置", "Fresh CLI unexpectedly reported a configured gateway");
            var providerError = await Failure(() => client.QueryAsync(["provider", "list", "--json"], CancellationToken.None));
            Assert(providerError.Message.Contains("provider configuration is missing"), "Missing configuration was not reported explicitly");
            Json.Rows(Json.Field(await client.QueryAsync(["agent", "list", "--json"], CancellationToken.None), "agents"));
            Json.Flag(await client.QueryAsync(["service", "autostart", "status", "--json"], CancellationToken.None), "enabled");
            Json.Flag(await client.QueryAsync(["ech", "status", "--json"], CancellationToken.None), "enabled");
            Json.Field(await client.QueryAsync(["fusion", "get", "--json"], CancellationToken.None), "profile");
        }
        finally
        {
            Environment.SetEnvironmentVariable("CODEX_GATEWAY_CONFIG", oldConfig);
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
