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

/// Chart granularity: local hours from reply rows for the one-day range, and
/// the CLI's UTC days for longer ranges.
enum UsageChartBucket {
    case hour
    case day

    var component: Calendar.Component {
        switch self {
        case .hour: return .hour
        case .day: return .day
        }
    }
}

/// One stacked chart segment: a time slot and a series (provider id, or the
/// aggregate label when providers are merged). `start` places the bar on the
/// local-time axis; `interval` is the slot the counts were taken from, which
/// for daily rows is the CLI's UTC day.
struct UsageChartPoint: Identifiable, Equatable {
    let start: Date
    let interval: DateInterval
    let series: String
    let tokens: UInt64
    let requests: UInt64
    var id: String { "\(start.timeIntervalSince1970)-\(series)" }
}

/// One point per slot in the range and per series, so slots without traffic
/// still appear on the chart axis. Daily rows come from the CLI's UTC-day
/// totals; the hourly view sums reply rows into local hours.
func usageChartPoints(
    daily rows: [DailyUsageRecord],
    turns: [TurnUsageRecord],
    bucket: UsageChartBucket,
    split: Bool,
    aggregateLabel: String,
    lastDay: Int,
    days: Int
) -> [UsageChartPoint] {
    var utc = Calendar(identifier: .gregorian)
    utc.timeZone = TimeZone(identifier: "UTC") ?? .current
    let local = Calendar.current
    let firstDay = lastDay - max(days, 1) + 1
    let rangeStart = Date(timeIntervalSince1970: Double(firstDay) * 86_400)

    // Slot interval -> axis position. A UTC day is drawn on the local date
    // with the same calendar day, so its label never shifts by the offset.
    var slots: [(interval: DateInterval, start: Date)] = []
    switch bucket {
    case .hour:
        var cursor = local.dateInterval(of: .hour, for: rangeStart)?.start ?? rangeStart
        while cursor <= Date() {
            let next = local.date(byAdding: .hour, value: 1, to: cursor) ?? cursor.addingTimeInterval(3_600)
            slots.append((DateInterval(start: cursor, end: next), cursor))
            cursor = next
        }
    case .day:
        for day in firstDay...lastDay {
            let start = Date(timeIntervalSince1970: Double(day) * 86_400)
            let parts = utc.dateComponents([.year, .month, .day], from: start)
            slots.append((DateInterval(start: start, duration: 86_400), local.date(from: parts) ?? start))
        }
    }

    var totals: [String: (tokens: UInt64, requests: UInt64)] = [:]
    var series = Set<String>()
    func add(_ name: String, at date: Date, tokens: UInt64, requests: UInt64) {
        guard let slot = slots.first(where: { $0.interval.start <= date && date < $0.interval.end }) else { return }
        series.insert(name)
        let key = "\(slot.interval.start.timeIntervalSince1970)|\(name)"
        let current = totals[key] ?? (0, 0)
        totals[key] = (current.tokens + tokens, current.requests + requests)
    }
    switch bucket {
    case .hour:
        for turn in turns {
            let tokens = (turn.inputTokens ?? 0) + (turn.cacheReadTokens ?? 0)
                + (turn.cacheCreationTokens ?? 0) + (turn.outputTokens ?? 0)
            add(split ? turn.providerID : aggregateLabel, at: turn.date, tokens: tokens, requests: 1)
        }
    case .day:
        for row in rows {
            let date = Date(timeIntervalSince1970: Double(row.day) * 86_400)
            add(split ? row.providerID : aggregateLabel, at: date, tokens: row.totalTokens, requests: row.requestCount)
        }
    }
    if series.isEmpty { series.insert(aggregateLabel) }
    return slots.flatMap { slot in
        series.sorted().map { name in
            let total = totals["\(slot.interval.start.timeIntervalSince1970)|\(name)"] ?? (0, 0)
            return UsageChartPoint(
                start: slot.start,
                interval: slot.interval,
                series: name,
                tokens: total.tokens,
                requests: total.requests
            )
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
