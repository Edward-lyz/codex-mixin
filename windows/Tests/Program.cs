using CodexMixin;

// End-to-end contract check: the Windows adapter parses the real Rust CLI.
if (args is not [var cliArgument])
{
    Console.Error.WriteLine("usage: Tests <path to codex-mixin.exe>");
    return 2;
}

void Assert(bool condition, string message) { if (!condition) throw new InvalidOperationException(message); }

var directory = Path.Combine(Path.GetTempPath(), "mixin-contract-" + Guid.NewGuid().ToString("N"));
Directory.CreateDirectory(directory);
Environment.SetEnvironmentVariable("CODEX_GATEWAY_CONFIG", Path.Combine(directory, "config.json"));
// No ChatGPT OAuth session may leak in from the runner's real Codex home.
Environment.SetEnvironmentVariable("CODEX_HOME", Path.Combine(directory, "codex"));
try
{
    var client = new CliClient(Path.GetFullPath(cliArgument));
    var metadata = await client.QueryAsync(["interface", "--json"], CancellationToken.None);
    Json.Interface(metadata);
    var catalog = Json.Field(metadata, "provider_catalog");
    var presets = Json.Rows(Json.Field(catalog, "presets"));
    var agy = presets.Single(row => Json.Text(row, "id") == "agy");
    Assert(Json.Text(agy, "auth_mode") == "local_oauth" && !Json.Flag(agy, "api_key_required"),
        "Antigravity must support local OAuth without an API key");
    var business = presets.Single(row => Json.Text(row, "id") == "agy-business");
    Assert(Json.Text(business, "auth_mode") == "local_oauth" && !Json.Flag(business, "api_key_required"),
        "Antigravity Business must support browser OAuth without an API key");
    var kiro = presets.Single(row => Json.Text(row, "id") == "kiro");
    Assert(Json.Text(kiro, "auth_mode") == "local_session" && !Json.Flag(kiro, "api_key_required"),
        "Kiro must support a local session without an API key");
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
    Console.WriteLine("PASS real Rust CLI interface and read-only contracts");
    return 0;
}
catch (Exception error)
{
    Console.Error.WriteLine($"FAIL real Rust CLI interface and read-only contracts: {error}");
    return 1;
}
finally
{
    Directory.Delete(directory, true);
}
