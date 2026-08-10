import Foundation

enum GatewayError: Error {
    case command(String)
}

@main
struct QuotaSupportTests {
    static func main() throws {
        let usages = try parseProviderQuotaUsage(
            """
            [
              {
                "provider_id": "custom-2",
                "display_name": "AIHub",
                "used": 0.2,
                "limit": 10,
                "remaining": 9.8
              },
              {
                "provider_id": "custom-3",
                "used": 1
              },
              {
                "provider_id": "deepseek",
                "display_name": "DeepSeek",
                "currency": "CNY",
                "used": null,
                "remaining": 110
              }
            ]
            """
        )

        precondition(usages[0].menuLabel == "AIHub")
        precondition(usages[1].menuLabel == "custom-3")
        precondition(usages[2].used == nil)
        precondition(usages[2].remaining == 110)
        precondition(usages[2].currency == "CNY")
        print("Provider quota labels: passed")

        let tokenUsages = try parseProviderTokenUsage(
            """
            [
              {
                "provider_id": "baidu-oneapi",
                "model_id": "gpt-5.6-sol",
                "request_count": 2,
                "input_tokens": 1500,
                "cache_read_tokens": 4500,
                "cache_creation_tokens": 500,
                "output_tokens": 300,
                "cache_hit_percent": 75.0
              }
            ]
            """
        )
        precondition(tokenUsages[0].providerID == "baidu-oneapi")
        precondition(tokenUsages[0].modelID == "gpt-5.6-sol")
        precondition(tokenUsages[0].requestCount == 2)
        precondition(tokenUsages[0].totalTokens == 6800)
        precondition(tokenUsages[0].cacheHitPercent == 75.0)
        print("Provider token usage parsing: passed")
    }
}
