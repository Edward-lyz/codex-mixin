using System.Text.Json;

namespace CodexMixin;

// Presentation-only aggregation: each CLI row is visited once, not once per day/series.
internal static class UsageBuckets
{
    internal static (string[] Series, long[] Days, long[][] Values, long Total, long Maximum) Build(
        JsonElement[] rows, int days, bool split, long today,
        Func<JsonElement, long> metric, Func<JsonElement, long> tokens)
    {
        ArgumentOutOfRangeException.ThrowIfNegativeOrZero(days);
        var order = new List<string>();
        var totals = new Dictionary<string, long>();
        var cells = new Dictionary<(long Day, string Series), long>();
        var total = 0L;
        foreach (var row in rows)
        {
            var key = split ? Json.Text(row, "provider_id") : "全部";
            if (!totals.ContainsKey(key)) { order.Add(key); totals[key] = 0; }
            totals[key] += tokens(row);
            var value = metric(row);
            total += value;
            var day = row.TryGetProperty("day", out var stamp) && stamp.ValueKind == JsonValueKind.Number ? stamp.GetInt64() : 0;
            var cell = (day, key);
            cells[cell] = cells.GetValueOrDefault(cell) + value;
        }
        var series = order.OrderByDescending(key => totals[key]).ToArray();
        var buckets = Enumerable.Range(0, days).Select(offset => today - days + 1 + offset).ToArray();
        var values = buckets.Select(day => series.Select(key => cells.GetValueOrDefault((day, key))).ToArray()).ToArray();
        var maximum = Math.Max(1, values.Max(row => row.Sum()));
        return (series, buckets, values, total, maximum);
    }
}
