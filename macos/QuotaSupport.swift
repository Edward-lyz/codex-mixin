import Foundation

func parseQuotaUsage(_ rawJson: String) throws -> QuotaUsage {
    let data = Data(rawJson.utf8)
    guard let root = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw GatewayError.command("额度接口返回的 JSON 不是对象")
    }
    for payload in quotaPayloads(root) {
        guard let used = firstNumericValue(payload, ["used", "used_quota", "usage", "total_usage", "spent", "cost", "consumed"]) else {
            continue
        }
        let limit = firstNumericValue(payload, ["limit", "total", "total_credits", "quota", "quota_limit", "month_quota_limit", "budget"])
        let remaining = firstNumericValue(payload, ["remaining", "remaining_quota", "available"])
        return QuotaUsage(used: used, limit: limit, remaining: remaining)
    }
    throw GatewayError.command("额度接口返回缺少 used 字段")
}

func quotaPayloads(_ root: [String: Any]) -> [[String: Any]] {
    var payloads = [root]
    if let data = root["data"] as? [String: Any] {
        payloads.append(data)
        if let quota = data["quota"] as? [String: Any] {
            payloads.append(quota)
        }
        if let usage = data["usage"] as? [String: Any] {
            payloads.append(usage)
        }
    }
    if let quota = root["quota"] as? [String: Any] {
        payloads.append(quota)
    }
    if let usage = root["usage"] as? [String: Any] {
        payloads.append(usage)
    }
    return payloads
}

func firstNumericValue(_ payload: [String: Any], _ keys: [String]) -> Double? {
    for key in keys {
        if let value = numericValue(payload[key]) {
            return value
        }
    }
    return nil
}

func numericValue(_ value: Any?) -> Double? {
    if let number = value as? NSNumber {
        return number.doubleValue
    }
    if let string = value as? String {
        return Double(string)
    }
    return nil
}

