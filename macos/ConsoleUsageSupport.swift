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
    /// `completed` or `failed`; gateways that predate status omit it.
    let status: String?
    let error: String?

    var id: String { "\(atMs)-\(session)-\(providerID)-\(upstreamModelID)-\(status ?? "")" }
    var date: Date { Date(timeIntervalSince1970: Double(atMs) / 1_000) }
    var isFailed: Bool { status == "failed" }
    /// The CLI already treats aliases and dated snapshots as the same model,
    /// so `false` here means the provider served a different model.
    var isSubstituted: Bool { modelMatches == false && returnedModel != nil }

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
        case status
        case error
    }

    /// Long prompts that mostly missed the cache. Short prompts are too small
    /// for provider caches, matching the gateway's own 8192-token threshold.
    var cacheIsLow: Bool {
        let prompt = (inputTokens ?? 0) + (cacheReadTokens ?? 0) + (cacheCreationTokens ?? 0)
        guard let percent = cacheHitPercent else { return false }
        return prompt >= 8_192 && percent < 50
    }

    var stats: UsageSlotStats {
        var stats = UsageSlotStats()
        if isFailed { stats.errors = 1 } else { stats.requests = 1 }
        stats.cached = cacheReadTokens ?? 0
        stats.uncached = (inputTokens ?? 0) + (cacheCreationTokens ?? 0)
        stats.output = outputTokens ?? 0
        if cacheHitPercent != nil {
            stats.hitRead = stats.cached
            stats.hitPrompt = stats.cached + stats.uncached
        }
        return stats
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
    /// Absent from gateways that predate failure accounting.
    let errorCount: UInt64?

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
        case errorCount = "error_count"
    }

    var stats: UsageSlotStats {
        var stats = UsageSlotStats()
        stats.requests = requestCount
        stats.errors = errorCount ?? 0
        stats.cached = cacheReadTokens
        stats.uncached = inputTokens + cacheCreationTokens
        stats.output = outputTokens
        if cacheHitPercent != nil {
            stats.hitRead = cacheReadTokens
            stats.hitPrompt = cacheReadTokens + inputTokens + cacheCreationTokens
        }
        return stats
    }
}

/// Counters for one period. `requests` are replies that reported usage and
/// `errors` are failed replies, so every rate shares the attempt total.
struct UsageSlotStats: Equatable {
    var requests: UInt64 = 0
    var errors: UInt64 = 0
    var cached: UInt64 = 0
    var uncached: UInt64 = 0
    var output: UInt64 = 0
    /// Prompt counters of providers that report cache usage, for the hit rate.
    var hitRead: UInt64 = 0
    var hitPrompt: UInt64 = 0

    init() {}

    init<S: Sequence>(_ items: S) where S.Element == UsageSlotStats {
        for item in items { self += item }
    }

    static func += (left: inout UsageSlotStats, right: UsageSlotStats) {
        left.requests += right.requests
        left.errors += right.errors
        left.cached += right.cached
        left.uncached += right.uncached
        left.output += right.output
        left.hitRead += right.hitRead
        left.hitPrompt += right.hitPrompt
    }

    var attempts: UInt64 { requests + errors }
    var tokens: UInt64 { cached + uncached + output }
    var cacheHitPercent: Double? { hitPrompt > 0 ? Double(hitRead) / Double(hitPrompt) * 100 : nil }
    var errorPercent: Double? { attempts > 0 ? Double(errors) / Double(attempts) * 100 : nil }
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

/// One chart slot. `start` places the bar on the local-time axis; `interval`
/// is the period the counts were taken from, which for daily rows is the
/// CLI's UTC day.
struct UsageSlot: Identifiable, Equatable {
    let start: Date
    let interval: DateInterval
    var total = UsageSlotStats()
    var byProvider: [String: UsageSlotStats] = [:]
    var id: TimeInterval { start.timeIntervalSince1970 }
}

/// One slot per period in the range, so periods without traffic still appear
/// on the chart axis. Daily rows come from the CLI's UTC-day totals; the
/// hourly view sums reply rows into local hours.
func usageSlots(
    daily rows: [DailyUsageRecord],
    turns: [TurnUsageRecord],
    bucket: UsageChartBucket,
    lastDay: Int,
    days: Int
) -> [UsageSlot] {
    var utc = Calendar(identifier: .gregorian)
    utc.timeZone = TimeZone(identifier: "UTC") ?? .current
    let local = Calendar.current
    let firstDay = lastDay - max(days, 1) + 1
    let rangeStart = Date(timeIntervalSince1970: Double(firstDay) * 86_400)

    // A UTC day is drawn on the local date with the same calendar day, so
    // its label never shifts by the offset.
    var slots: [UsageSlot] = []
    switch bucket {
    case .hour:
        var cursor = local.dateInterval(of: .hour, for: rangeStart)?.start ?? rangeStart
        while cursor <= Date() {
            let next = local.date(byAdding: .hour, value: 1, to: cursor) ?? cursor.addingTimeInterval(3_600)
            slots.append(UsageSlot(start: cursor, interval: DateInterval(start: cursor, end: next)))
            cursor = next
        }
    case .day:
        for day in firstDay...lastDay {
            let start = Date(timeIntervalSince1970: Double(day) * 86_400)
            let parts = utc.dateComponents([.year, .month, .day], from: start)
            slots.append(UsageSlot(start: local.date(from: parts) ?? start, interval: DateInterval(start: start, duration: 86_400)))
        }
    }
    func add(_ stats: UsageSlotStats, provider: String, at date: Date) {
        guard let index = slots.firstIndex(where: { $0.interval.start <= date && date < $0.interval.end }) else { return }
        slots[index].total += stats
        slots[index].byProvider[provider, default: UsageSlotStats()] += stats
    }
    switch bucket {
    case .hour:
        for turn in turns { add(turn.stats, provider: turn.providerID, at: turn.date) }
    case .day:
        for row in rows {
            add(row.stats, provider: row.providerID, at: Date(timeIntervalSince1970: Double(row.day) * 86_400))
        }
    }
    return slots
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
