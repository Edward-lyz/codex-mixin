import Foundation

/// One upstream reply from `usage --turns --json`. Optional counters stay nil
/// when the provider did not report them, so they are never shown as zero.
struct TurnUsageRecord: Decodable, Identifiable, Equatable {
    let atMs: UInt64
    let session: String
    let sessionDisplay: String?
    let providerID: String
    let catalogSlug: String
    let upstreamModelID: String
    let returnedModel: String?
    let modelMatches: Bool?
    let inputTokens: UInt64?
    let cacheReadTokens: UInt64?
    let cacheCreationTokens: UInt64?
    let outputTokens: UInt64?
    let reasoningTokens: UInt64?
    let reasoningChars: UInt64
    let cacheHitPercent: Double?
    let ttftMs: Double?
    let outputTPS: Double?

    var id: String { "\(atMs)-\(session)-\(providerID)-\(upstreamModelID)" }
    var date: Date { Date(timeIntervalSince1970: Double(atMs) / 1_000) }

    enum CodingKeys: String, CodingKey {
        case atMs = "at_ms"
        case session
        case sessionDisplay = "session_display"
        case providerID = "provider_id"
        case catalogSlug = "catalog_slug"
        case upstreamModelID = "upstream_model_id"
        case returnedModel = "returned_model"
        case modelMatches = "model_matches"
        case inputTokens = "input_tokens"
        case cacheReadTokens = "cache_read_tokens"
        case cacheCreationTokens = "cache_creation_tokens"
        case outputTokens = "output_tokens"
        case reasoningTokens = "reasoning_tokens"
        case reasoningChars = "reasoning_chars"
        case cacheHitPercent = "cache_hit_percent"
        case ttftMs = "ttft_ms"
        case outputTPS = "output_tps"
    }

    /// Long prompts that mostly missed the cache. Short prompts are too small
    /// for provider caches, matching the gateway's own 8192-token threshold.
    var cacheIsLow: Bool {
        let prompt = (inputTokens ?? 0) + (cacheReadTokens ?? 0) + (cacheCreationTokens ?? 0)
        guard let percent = cacheHitPercent else { return false }
        return prompt >= 8_192 && percent < 50
    }

    var reasoningText: String {
        if let reasoningTokens { return formatTokenCount(reasoningTokens) }
        // Anthropic-style upstreams stream thinking without a token counter.
        return reasoningChars > 0 ? AppLocalization.string("console.usage.chars", formatTokenCount(reasoningChars)) : "—"
    }
}

/// One provider/model total for a UTC day from `usage --daily --json`.
struct DailyUsageRecord: Decodable, Equatable {
    let day: Int
    let providerID: String
    let modelID: String
    let requestCount: UInt64
    let inputTokens: UInt64
    let cacheReadTokens: UInt64
    let cacheCreationTokens: UInt64
    let outputTokens: UInt64
    let cacheHitPercent: Double?

    enum CodingKeys: String, CodingKey {
        case day
        case providerID = "provider_id"
        case modelID = "model_id"
        case requestCount = "request_count"
        case inputTokens = "input_tokens"
        case cacheReadTokens = "cache_read_tokens"
        case cacheCreationTokens = "cache_creation_tokens"
        case outputTokens = "output_tokens"
        case cacheHitPercent = "cache_hit_percent"
    }

    var totalTokens: UInt64 { inputTokens + cacheReadTokens + cacheCreationTokens + outputTokens }
}

/// Totals shown above the charts for the current filter.
struct UsageTotals: Equatable {
    var requests: UInt64 = 0
    var tokens: UInt64 = 0
    var output: UInt64 = 0
    var cached: UInt64 = 0
    var prompt: UInt64 = 0

    init(_ rows: [DailyUsageRecord]) {
        for row in rows {
            requests += row.requestCount
            tokens += row.totalTokens
            output += row.outputTokens
            if row.cacheHitPercent != nil {
                cached += row.cacheReadTokens
                prompt += row.cacheReadTokens + row.inputTokens + row.cacheCreationTokens
            }
        }
    }

    var cacheHitPercent: Double? { prompt > 0 ? Double(cached) / Double(prompt) * 100 : nil }
}

/// One stacked chart segment: a day and a series (provider id, or nil when
/// all providers are aggregated).
struct UsageChartPoint: Identifiable, Equatable {
    let day: Int
    let series: String
    let tokens: UInt64
    let requests: UInt64
    var id: String { "\(day)-\(series)" }

    /// Month/day label; days are UTC like the CLI's daily buckets.
    var dayLabel: String {
        let date = Date(timeIntervalSince1970: Double(day) * 86_400)
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? .current
        let parts = calendar.dateComponents([.month, .day], from: date)
        return "\(parts.month ?? 0)/\(parts.day ?? 0)"
    }
}

/// One point per day in the range and per series, so days without traffic
/// still appear as empty slots on the chart axis.
func usageChartPoints(
    _ rows: [DailyUsageRecord],
    split: Bool,
    aggregateLabel: String,
    lastDay: Int,
    days: Int
) -> [UsageChartPoint] {
    var totals: [String: (UInt64, UInt64)] = [:]
    var series = Set<String>()
    for row in rows {
        let name = split ? row.providerID : aggregateLabel
        series.insert(name)
        let key = "\(row.day)|\(name)"
        let current = totals[key] ?? (0, 0)
        totals[key] = (current.0 + row.totalTokens, current.1 + row.requestCount)
    }
    if series.isEmpty { series.insert(aggregateLabel) }
    let firstDay = lastDay - max(days, 1) + 1
    return (firstDay...lastDay).flatMap { day in
        series.sorted().map { name in
            let total = totals["\(day)|\(name)"] ?? (0, 0)
            return UsageChartPoint(day: day, series: name, tokens: total.0, requests: total.1)
        }
    }
}

func currentUnixDay(_ now: Date = Date()) -> Int {
    Int(now.timeIntervalSince1970 / 86_400)
}

func parseTurnUsage(_ rawJSON: String) throws -> [TurnUsageRecord] {
    do {
        return try JSONDecoder().decode([TurnUsageRecord].self, from: Data(rawJSON.utf8))
    } catch {
        throw GatewayError.command("回复明细 JSON 无法解析：\(error)")
    }
}

func parseDailyUsage(_ rawJSON: String) throws -> [DailyUsageRecord] {
    do {
        return try JSONDecoder().decode([DailyUsageRecord].self, from: Data(rawJSON.utf8))
    } catch {
        throw GatewayError.command("每日用量 JSON 无法解析：\(error)")
    }
}
