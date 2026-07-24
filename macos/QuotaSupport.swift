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
