using System.Globalization;
using System.Text.Json;

namespace CodexMixin;

// Snapshot of the persisted context settings behind "context config get --json".
// max_tier and guidance_only are not edited by this shell yet, but they are still
// parsed so a changed CLI contract fails loudly instead of falling back quietly.
internal sealed record ContextSettings(
    bool Enabled,
    long GrowthThresholdTokens,
    double PressureRatio,
    double UrgentRatio,
    long ProtectedMessages,
    long ProtectedTokens,
    long MaxInternalRounds,
    long MaxTier,
    bool GuidanceOnly,
    long? ContextWindowTokens);

internal sealed record ContextConfig(ContextSettings Settings, string ConfigPath, string DatabasePath);

internal sealed record ContextCache(long Turns, long InputTokens, long CachedTokens, long OutputTokens, double HitPercent);

internal sealed record ContextSession(
    string Session,
    string Display,
    long Revision,
    long UpdatedAt,
    long Messages,
    long Compressions,
    long SavedTokens,
    long ViewTokens,
    long TierMessages,
    long Tier1Blocks,
    long Tier2Blocks,
    long Tier3Blocks,
    ContextCache Cache);

internal sealed record ContextTotals(long Sessions, long Messages, long Compressions, long SavedTokens, long ViewTokens, ContextCache Cache);

internal sealed record ContextStats(bool Enabled, string DatabasePath, ContextTotals Totals, ContextSession[] Sessions);

// Strict readers for the context CLI contract. Display decisions stay in the page.
internal static class ContextContract
{
    internal static ContextConfig ParseConfig(JsonElement value) => new(
        ParseSettings(Json.Field(value, "context")),
        Json.Text(value, "config_path"),
        Json.Text(value, "database_path"));

    internal static ContextSettings ParseSettings(JsonElement value) => new(
        Json.Flag(value, "enabled"),
        Json.Integer(value, "growth_threshold_tokens"),
        Json.Real(value, "pressure_ratio"),
        Json.Real(value, "urgent_ratio"),
        Json.Integer(value, "protected_messages"),
        Json.Integer(value, "protected_tokens"),
        Json.Integer(value, "max_internal_rounds"),
        Json.Integer(value, "max_tier"),
        Json.Flag(value, "guidance_only"),
        Json.OptionalInteger(value, "context_window_tokens"));

    internal static ContextStats ParseStats(JsonElement value) => new(
        Json.Flag(value, "enabled"),
        Json.Text(value, "database_path"),
        ParseTotals(Json.Field(value, "totals")),
        Json.Rows(Json.Field(value, "sessions")).Select(ParseSession).ToArray());

    /// <summary>
    /// One "context config set" call with exactly the edited flags; the CLI keeps
    /// every stored value that is not supplied. Null means nothing changed.
    /// </summary>
    internal static string[]? SaveArguments(ContextSettings stored, ContextSettings edited)
    {
        var arguments = new List<string> { "context", "config", "set" };
        if (edited.Enabled != stored.Enabled) arguments.AddRange(["--enabled", edited.Enabled ? "true" : "false"]);
        if (edited.GrowthThresholdTokens != stored.GrowthThresholdTokens) arguments.AddRange(["--growth-threshold-tokens", Number(edited.GrowthThresholdTokens)]);
        if (edited.PressureRatio != stored.PressureRatio) arguments.AddRange(["--pressure-ratio", Number(edited.PressureRatio)]);
        if (edited.UrgentRatio != stored.UrgentRatio) arguments.AddRange(["--urgent-ratio", Number(edited.UrgentRatio)]);
        if (edited.ProtectedMessages != stored.ProtectedMessages) arguments.AddRange(["--protected-messages", Number(edited.ProtectedMessages)]);
        if (edited.ProtectedTokens != stored.ProtectedTokens) arguments.AddRange(["--protected-tokens", Number(edited.ProtectedTokens)]);
        if (edited.MaxInternalRounds != stored.MaxInternalRounds) arguments.AddRange(["--max-internal-rounds", Number(edited.MaxInternalRounds)]);
        if (edited.ContextWindowTokens != stored.ContextWindowTokens)
            arguments.AddRange(["--context-window-tokens", edited.ContextWindowTokens is { } tokens ? Number(tokens) : "none"]);
        return arguments.Count == 3 ? null : arguments.ToArray();
    }

    private static ContextSession ParseSession(JsonElement row)
    {
        var tiers = Json.Field(row, "tiers");
        return new ContextSession(
            Json.Text(row, "session"),
            Json.Text(row, "display"),
            Json.Integer(row, "revision"),
            Json.Integer(row, "updated_at"),
            Json.Integer(row, "messages"),
            Json.Integer(row, "compressions"),
            Json.Integer(row, "saved_tokens"),
            Json.Integer(row, "view_tokens"),
            Json.Integer(tiers, "messages"),
            Json.Integer(tiers, "t1_blocks"),
            Json.Integer(tiers, "t2_blocks"),
            Json.Integer(tiers, "t3_blocks"),
            ParseCache(Json.Field(row, "cache")));
    }

    private static ContextTotals ParseTotals(JsonElement value) => new(
        Json.Integer(value, "sessions"),
        Json.Integer(value, "messages"),
        Json.Integer(value, "compressions"),
        Json.Integer(value, "saved_tokens"),
        Json.Integer(value, "view_tokens"),
        ParseCache(Json.Field(value, "cache")));

    private static ContextCache ParseCache(JsonElement value)
    {
        var cache = new ContextCache(
            Json.Integer(value, "turns"),
            Json.Integer(value, "input_tokens"),
            Json.Integer(value, "cached_tokens"),
            Json.Integer(value, "output_tokens"),
            Json.Real(value, "hit_percent"));
        // The CLI derives the share from billed input tokens; anything outside
        // 0..100 is not the contract this shell was written against.
        return cache.HitPercent is >= 0 and <= 100 ? cache
            : throw new CliFailure("CLI contract 字段 hit_percent 必须在 0 到 100 之间。");
    }

    private static string Number(long value) => value.ToString(CultureInfo.InvariantCulture);
    private static string Number(double value) => value.ToString(CultureInfo.InvariantCulture);
}
