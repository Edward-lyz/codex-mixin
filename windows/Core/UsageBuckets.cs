using System.Text.Json;

namespace CodexMixin;

/// Counters for one period. `Requests` are replies that reported usage and
/// `Errors` are failed replies, so every rate shares the attempt total.
internal readonly record struct UsageStats(long Requests, long Errors, long Cached, long Uncached, long Output, long HitRead, long HitPrompt, double Cost = 0, long Costed = 0, long Unpriced = 0)
{
    internal long Attempts => Requests + Errors;
    internal long Tokens => Cached + Uncached + Output;
    internal double? CacheHitPercent => HitPrompt > 0 ? HitRead * 100.0 / HitPrompt : null;
    internal double? ErrorPercent => Attempts > 0 ? Errors * 100.0 / Attempts : null;

    public static UsageStats operator +(UsageStats left, UsageStats right) => new(
        checked(left.Requests + right.Requests), checked(left.Errors + right.Errors),
        checked(left.Cached + right.Cached), checked(left.Uncached + right.Uncached), checked(left.Output + right.Output),
        checked(left.HitRead + right.HitRead), checked(left.HitPrompt + right.HitPrompt),
        left.Cost + right.Cost, checked(left.Costed + right.Costed), checked(left.Unpriced + right.Unpriced));

    internal static UsageStats Sum(IEnumerable<UsageStats> items) => items.Aggregate(default(UsageStats), (sum, item) => sum + item);

    private static long Count(JsonElement row, string key) =>
        row.TryGetProperty(key, out var value) && value.ValueKind == JsonValueKind.Number ? value.GetInt64() : 0;

    private static double? CostEstimate(JsonElement row) =>
        row.TryGetProperty("estimated_cost_usd", out var value) && value.ValueKind == JsonValueKind.Number
        && value.TryGetDouble(out var cost) && double.IsFinite(cost) && cost >= 0 ? cost : null;

    private static bool Reports(JsonElement row, string key) =>
        row.TryGetProperty(key, out var value) && value.ValueKind == JsonValueKind.Number;

    /// One provider/model total for a UTC day from `usage --daily --json`.
    internal static UsageStats Daily(JsonElement row)
    {
        var cached = Count(row, "cache_read_tokens");
        var uncached = Count(row, "input_tokens") + Count(row, "cache_creation_tokens");
        var reports = Reports(row, "cache_hit_percent");
        var requests = Count(row, "request_count");
        var cost = CostEstimate(row);
        var unpriced = cost.HasValue ? Math.Min(requests, Count(row, "unpriced_request_count")) : requests;
        return new(requests, Count(row, "error_count"), cached, uncached, Count(row, "output_tokens"),
            reports ? cached : 0, reports ? cached + uncached : 0, cost ?? 0,
            cost.HasValue ? requests - unpriced : 0, unpriced);
    }

    /// One reply from `usage --turns --json`.
    internal static UsageStats Turn(JsonElement turn)
    {
        var failed = IsFailed(turn);
        var cached = Count(turn, "cache_read_tokens");
        var uncached = Count(turn, "input_tokens") + Count(turn, "cache_creation_tokens");
        var reports = Reports(turn, "cache_hit_percent");
        var cost = failed ? null : CostEstimate(turn);
        return new(failed ? 0 : 1, failed ? 1 : 0, cached, uncached, Count(turn, "output_tokens"),
            reports ? cached : 0, reports ? cached + uncached : 0,
            cost ?? 0, cost.HasValue ? 1 : 0, !failed && !cost.HasValue ? 1 : 0);
    }

    internal static bool IsFailed(JsonElement turn) => Json.OptionalText(turn, "status") == "failed";

    /// The upstream answered with a different model; vendor aliases already match in the CLI.
    internal static bool IsSubstituted(JsonElement turn) =>
        turn.TryGetProperty("model_matches", out var match) && match.ValueKind == JsonValueKind.False
        && Json.OptionalText(turn, "returned_model") != null;
}

/// One chart slot. `Start`/`End` is the period the counts were taken from: a
/// local hour for the one-day range, otherwise the CLI's UTC day.
internal sealed class UsageSlot(DateTimeOffset start, DateTimeOffset end)
{
    internal DateTimeOffset Start { get; } = start;
    internal DateTimeOffset End { get; } = end;
    internal UsageStats Total { get; set; }
    internal Dictionary<string, UsageStats> ByProvider { get; } = [];
    internal bool Hourly => End - Start <= TimeSpan.FromHours(1);
}

// Presentation-only aggregation: each CLI row is visited once.
internal static class UsageBuckets
{
    internal const long DayMs = 86_400_000;

    internal static long Today(DateTimeOffset now) => now.ToUnixTimeMilliseconds() / DayMs;

    /// One slot per period in the range, so periods without traffic still
    /// appear on the axis. Longer ranges use the CLI's UTC-day totals; the
    /// one-day range sums reply rows into local hours.
    internal static UsageSlot[] Slots(JsonElement[] daily, JsonElement[] turns, int days, DateTimeOffset now)
    {
        ArgumentOutOfRangeException.ThrowIfNegativeOrZero(days);
        var firstDay = Today(now) - days + 1;
        var slots = new List<UsageSlot>();
        if (days == 1)
        {
            var start = DateTimeOffset.FromUnixTimeMilliseconds(firstDay * DayMs).ToLocalTime();
            var cursor = new DateTimeOffset(start.Year, start.Month, start.Day, start.Hour, 0, 0, start.Offset);
            while (cursor <= now) { slots.Add(new UsageSlot(cursor, cursor.AddHours(1))); cursor = cursor.AddHours(1); }
            foreach (var turn in turns)
                Add(slots, UsageStats.Turn(turn), Json.Text(turn, "provider_id"),
                    DateTimeOffset.FromUnixTimeMilliseconds(Number(turn, "at_ms")));
        }
        else
        {
            for (var day = firstDay; day < firstDay + days; day++)
                slots.Add(new UsageSlot(DateTimeOffset.FromUnixTimeMilliseconds(day * DayMs), DateTimeOffset.FromUnixTimeMilliseconds((day + 1) * DayMs)));
            foreach (var row in daily)
                Add(slots, UsageStats.Daily(row), Json.Text(row, "provider_id"),
                    DateTimeOffset.FromUnixTimeMilliseconds(Number(row, "day") * DayMs));
        }
        return slots.ToArray();
    }

    private static long Number(JsonElement row, string key) =>
        row.TryGetProperty(key, out var value) && value.ValueKind == JsonValueKind.Number ? value.GetInt64() : 0;

    private static void Add(List<UsageSlot> slots, UsageStats stats, string provider, DateTimeOffset at)
    {
        var slot = slots.FirstOrDefault(slot => slot.Start <= at && at < slot.End);
        if (slot == null) return;
        slot.Total += stats;
        slot.ByProvider[provider] = slot.ByProvider.GetValueOrDefault(provider) + stats;
    }

    /// Daily rows of the range and of the equal-length period before it.
    internal static (JsonElement[] Current, JsonElement[] Previous) SplitPeriods(JsonElement[] rows, int days, DateTimeOffset now)
    {
        var firstDay = Today(now) - days + 1;
        return (rows.Where(row => Number(row, "day") >= firstDay).ToArray(), rows.Where(row => Number(row, "day") < firstDay).ToArray());
    }
}
