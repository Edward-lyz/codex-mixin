using System.Text.Json;

namespace CodexMixin;

// A shell-owned display snapshot, never a substitute for CLI configuration.
// Only presentation fields are persisted; raw upstream/auth data is excluded.
internal sealed record QuotaDisplayCache(JsonElement[] Providers, JsonElement[] Rows, Dictionary<string, DateTimeOffset> UpdatedAt)
{
    private const int MaximumBytes = 1024 * 1024;
    private static readonly string[] ProviderFields = ["id", "display_name", "icon", "preset_id"];
    private static readonly string[] TextFields = ["provider_id", "quota_id", "label", "unit", "currency", "reset_at"];
    private static readonly string[] AmountFields = ["used", "value", "limit", "remaining"];

    internal static QuotaDisplayCache Create(JsonElement[] providers, JsonElement[] rows, Dictionary<string, DateTimeOffset> updatedAt)
    {
        if (providers.Length > 128 || rows.Length > 512) throw new InvalidDataException("额度展示缓存过大。");
        var safeProviders = providers.Select(row => Project(row, ProviderFields, [])).ToArray();
        var ids = safeProviders.Select(row => Json.Text(row, "id")).ToHashSet(StringComparer.Ordinal);
        var safeRows = rows.Where(row => Json.OptionalText(row, "error") == null && ids.Contains(Json.Text(row, "provider_id")))
            .Select(row => Project(row, TextFields, AmountFields)).ToArray();
        return new(safeProviders, safeRows, updatedAt.Where(pair => ids.Contains(pair.Key)).ToDictionary());
    }

    private static JsonElement Project(JsonElement row, string[] textFields, string[] amountFields)
    {
        var result = new Dictionary<string, object?>();
        foreach (var key in textFields)
        {
            var text = Json.OptionalText(row, key);
            if (text?.Length > 1024) throw new InvalidDataException("额度展示字段过长。");
            if (text != null) result[key] = text;
        }
        foreach (var key in amountFields)
        {
            if (!row.TryGetProperty(key, out var value) || value.ValueKind == JsonValueKind.Null) continue;
            if (value.ValueKind != JsonValueKind.Number || !value.TryGetDouble(out var amount) || !double.IsFinite(amount) || amount < 0)
                throw new InvalidDataException("额度展示数值无效。");
            result[key] = amount;
        }
        return JsonSerializer.SerializeToElement(result);
    }

    internal static Task<QuotaDisplayCache?> LoadAsync(string path, CancellationToken token) => Task.Run(async () =>
    {
        if (!File.Exists(path)) return null;
        await using var file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, true);
        if (file.Length > MaximumBytes) throw new InvalidDataException("额度展示缓存过大。");
        var cache = await JsonSerializer.DeserializeAsync<QuotaDisplayCache>(file, cancellationToken: token)
            ?? throw new InvalidDataException("额度展示缓存为空。");
        if (cache.Providers == null || cache.Rows == null || cache.UpdatedAt == null) throw new InvalidDataException("额度展示缓存不完整。");
        return Create(cache.Providers, cache.Rows, cache.UpdatedAt);
    }, token);

    internal Task SaveAsync(string path, CancellationToken token) => Task.Run(async () =>
    {
        var data = JsonSerializer.SerializeToUtf8Bytes(this);
        if (data.Length > MaximumBytes) throw new InvalidDataException("额度展示缓存过大。");
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        var temporary = path + ".tmp";
        try
        {
            await File.WriteAllBytesAsync(temporary, data, token);
            File.Move(temporary, path, true);
        }
        finally { if (File.Exists(temporary)) File.Delete(temporary); }
    }, token);
}
