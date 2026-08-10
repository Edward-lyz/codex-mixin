import Foundation

struct ProviderQuotaUsage: Decodable {
    let providerID: String
    let displayName: String?
    let currency: String?
    let used: Double?
    let limit: Double?
    let remaining: Double?
    let error: String?
    let staleAt: String?
    let resetAt: String?

    enum CodingKeys: String, CodingKey {
        case providerID = "provider_id"
        case displayName = "display_name"
        case currency
        case used
        case value
        case limit
        case remaining
        case error
        case staleAt = "stale_at"
        case resetAt = "reset_at"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        providerID = try values.decode(String.self, forKey: .providerID)
        displayName = try values.decodeIfPresent(String.self, forKey: .displayName)
        currency = try values.decodeIfPresent(String.self, forKey: .currency)
        used = try values.decodeIfPresent(Double.self, forKey: .used)
            ?? values.decodeIfPresent(Double.self, forKey: .value)
        limit = try values.decodeIfPresent(Double.self, forKey: .limit)
        remaining = try values.decodeIfPresent(Double.self, forKey: .remaining)
        error = try values.decodeIfPresent(String.self, forKey: .error)
        staleAt = try values.decodeIfPresent(String.self, forKey: .staleAt)
        resetAt = try values.decodeIfPresent(String.self, forKey: .resetAt)
    }

    var menuLabel: String {
        guard let displayName = displayName?.trimmingCharacters(in: .whitespacesAndNewlines),
              !displayName.isEmpty
        else {
            return providerID
        }
        return displayName
    }
}

func parseProviderQuotaUsage(_ rawJSON: String) throws -> [ProviderQuotaUsage] {
    do {
        return try JSONDecoder().decode([ProviderQuotaUsage].self, from: Data(rawJSON.utf8))
    } catch {
        throw GatewayError.command("Provider 额度 JSON 无法解析：\(error)")
    }
}

struct ProviderTokenUsage: Decodable {
    let providerID: String
    let requestCount: UInt64
    let inputTokens: UInt64
    let cacheReadTokens: UInt64
    let cacheCreationTokens: UInt64
    let outputTokens: UInt64
    let cacheHitPercent: Double?

    enum CodingKeys: String, CodingKey {
        case providerID = "provider_id"
        case requestCount = "request_count"
        case inputTokens = "input_tokens"
        case cacheReadTokens = "cache_read_tokens"
        case cacheCreationTokens = "cache_creation_tokens"
        case outputTokens = "output_tokens"
        case cacheHitPercent = "cache_hit_percent"
    }
}

func parseProviderTokenUsage(_ rawJSON: String) throws -> [ProviderTokenUsage] {
    do {
        return try JSONDecoder().decode([ProviderTokenUsage].self, from: Data(rawJSON.utf8))
    } catch {
        throw GatewayError.command("Token 使用 JSON 无法解析：\(error)")
    }
}
