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
                "provider_display_name": "AIHub",
                "display_name": "AIHub",
                "quota_id": "quota",
                "label": "Quota",
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
        precondition(usages[0].providerLabel == "AIHub")
        precondition(usages[1].menuLabel == "custom-3")
        precondition(usages[2].used == nil)
        precondition(usages[2].remaining == 110)
        precondition(usages[2].currency == "CNY")
        let structured = try parseProviderQuotaUsage(
            """
            [{
              "provider_id": "opencode-go",
              "provider_display_name": "OpenCode Go",
              "display_name": "OpenCode Go Weekly",
              "quota_id": "weekly",
              "label": "Weekly",
              "used": 42,
              "limit": 100
            }]
            """
        )
        precondition(structured[0].providerLabel == "OpenCode Go")
        precondition(structured[0].quotaID == "weekly")
        precondition(structured[0].label == "Weekly")
        let official = try parseProviderQuotaUsage(
            """
            [{
              "provider_id": "official",
              "provider_display_name": "OpenAI",
              "display_name": "OpenAI Codex 5h",
              "quota_id": "codex.primary",
              "label": "Codex · 5h",
              "used": 25,
              "limit": 100,
              "remaining": 75,
              "reset_at": "2024-11-07T00:00:00Z"
            }]
            """
        )
        precondition(official[0].providerID == "official")
        precondition(official[0].providerLabel == "OpenAI")
        precondition(official[0].quotaID == "codex.primary")
        precondition(official[0].label == "Codex · 5h")
        precondition(official[0].used == 25)
        precondition(official[0].limit == 100)
        precondition(official[0].resetAt == "2024-11-07T00:00:00Z")
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
