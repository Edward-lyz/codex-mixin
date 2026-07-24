import Foundation

struct ProviderQuotaUsage: Decodable {
    let providerID: String
    let currency: String?
    let value: Double?
    let error: String?
    let staleAt: String?

    enum CodingKeys: String, CodingKey {
        case providerID = "provider_id"
        case currency
        case value
        case error
        case staleAt = "stale_at"
    }
}

func parseProviderQuotaUsage(_ rawJSON: String) throws -> [ProviderQuotaUsage] {
    do {
        return try JSONDecoder().decode([ProviderQuotaUsage].self, from: Data(rawJSON.utf8))
    } catch {
        throw GatewayError.command("Provider 额度 JSON 无法解析：\(error)")
    }
}
