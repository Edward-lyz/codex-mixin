using System.Text.Json;

namespace CodexMixin;

internal static class Json
{
    internal static JsonElement Field(JsonElement row, string key)
    {
        if (row.ValueKind != JsonValueKind.Object || !row.TryGetProperty(key, out var value))
            throw new CliFailure($"CLI contract 缺少字段：{key}");
        return value;
    }

    internal static string Text(JsonElement row, string key)
    {
        var value = Field(row, key);
        return value.ValueKind == JsonValueKind.String ? value.GetString()!
            : throw new CliFailure($"CLI contract 字段 {key} 必须是 string。");
    }

    internal static string? OptionalText(JsonElement row, string key)
    {
        if (row.ValueKind != JsonValueKind.Object || !row.TryGetProperty(key, out var value) ||
            value.ValueKind == JsonValueKind.Null) return null;
        return value.ValueKind == JsonValueKind.String ? value.GetString()
            : throw new CliFailure($"CLI contract 字段 {key} 必须是 string 或 null。");
    }

    internal static bool Flag(JsonElement row, string key)
    {
        var value = Field(row, key);
        return value.ValueKind is JsonValueKind.True or JsonValueKind.False ? value.GetBoolean()
            : throw new CliFailure($"CLI contract 字段 {key} 必须是 bool。");
    }

    internal static long Integer(JsonElement row, string key)
    {
        var value = Field(row, key);
        return value.ValueKind == JsonValueKind.Number && value.TryGetInt64(out var number) ? number
            : throw new CliFailure($"CLI contract 字段 {key} 必须是整数。");
    }

    internal static long? OptionalInteger(JsonElement row, string key)
    {
        if (row.ValueKind != JsonValueKind.Object || !row.TryGetProperty(key, out var value) ||
            value.ValueKind == JsonValueKind.Null) return null;
        return value.ValueKind == JsonValueKind.Number && value.TryGetInt64(out var number) ? number
            : throw new CliFailure($"CLI contract 字段 {key} 必须是整数或 null。");
    }

    internal static double Real(JsonElement row, string key)
    {
        var value = Field(row, key);
        return value.ValueKind == JsonValueKind.Number && value.TryGetDouble(out var number) ? number
            : throw new CliFailure($"CLI contract 字段 {key} 必须是数字。");
    }

    internal static JsonElement[] Rows(JsonElement value)
    {
        if (value.ValueKind != JsonValueKind.Array) throw new CliFailure("CLI contract 必须返回 array。");
        return value.EnumerateArray().Select(row => row.ValueKind == JsonValueKind.Object ? row
            : throw new CliFailure("CLI contract 列表包含无效对象。")).ToArray();
    }

    internal static string[] Strings(JsonElement value)
    {
        if (value.ValueKind != JsonValueKind.Array) throw new CliFailure("CLI contract 必须返回 string array。");
        return value.EnumerateArray().Select(row => row.ValueKind == JsonValueKind.String ? row.GetString()!
            : throw new CliFailure("CLI contract 列表包含无效 string。")).ToArray();
    }

    internal static void Interface(JsonElement value)
    {
        if (Field(value, "protocol_version").GetInt32() != 1)
            throw new CliFailure("不支持此 CLI protocol version，请更新完整应用包。");
        Text(value, "version");
        var paths = Field(value, "paths");
        foreach (var key in new[] { "state", "gateway_log" })
            if (!Path.IsPathFullyQualified(Text(paths, key)))
                throw new CliFailure($"CLI 返回的 {key} 路径必须是绝对路径。");
        foreach (var key in new[] { "service_lifecycle", "agent_management", "config_apply" })
            if (!Flag(Field(value, "capabilities"), key))
                throw new CliFailure($"当前 CLI 不支持 {key}，请更新完整应用包。");
        var catalog = Field(value, "provider_catalog");
        var presets = Rows(Field(catalog, "presets"));
        var protocols = Rows(Field(catalog, "protocols"));
        if (!presets.Any(row => Text(row, "id") == Text(catalog, "default_preset")) ||
            !protocols.Any(row => Text(row, "id") == Text(catalog, "default_protocol")))
            throw new CliFailure("CLI provider catalog 的默认选项无效，请更新完整应用包。");
        Text(catalog, "default_aws_region");
        foreach (var preset in presets)
        {
            Text(preset, "display_name");
            if (Text(preset, "auth_mode") is not ("api_key" or "local_oauth" or "aws_sigv4"))
                throw new CliFailure("CLI provider catalog 包含不支持的认证方式。");
            foreach (var key in new[] { "api_key_required", "custom_endpoint", "quota_username_required", "quota_credentials_required" })
                Flag(preset, key);
        }
        foreach (var protocol in protocols)
        {
            Text(protocol, "display_name");
            Text(protocol, "api_path");
        }
    }

    internal static string Gateway(JsonElement value)
    {
        // A fresh installation has no gateway field. It is unconfigured, not stopped.
        if (!Flag(value, "configured")) return "未配置";
        return Text(value, "gateway") switch
        {
            "running" => "运行中", "stopped" => "已停止",
            var unknown => throw new CliFailure($"未知网关状态：{unknown}")
        };
    }
}
