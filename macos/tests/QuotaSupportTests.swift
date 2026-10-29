import Foundation

enum GatewayError: Error {
    case command(String)
}

@main
struct QuotaSupportTests {
    static func main() throws {
        for (count, expected): (UInt64, String) in [
            (1_500_000_000, "1.5B"),
            (999, "999"), (1_000, "1.0K"), (999_949, "999.9K"),
            (999_950, "1.0M"), (999_950_000, "1.0B"),
            (1_000_000_000_000, "1.0T"),
            (UInt64.max, "18.4E"),
        ] {
            precondition(formatTokenCount(count) == expected, "\(count): \(formatTokenCount(count)) != \(expected)")
        }
        let dynamic = try parseProviderQuotaUsage(#"[{"provider_id":"agy","quota_id":"Future/Daily","label":"Future Daily","unit":"percent","remaining":42,"limit":100,"used":58,"reset_at":"2026-10-16T00:00:00Z"}]"#)
        precondition(dynamic[0].unit == "percent" && dynamic[0].remaining == 42)
        precondition(dynamic[0].resetAt == "2026-10-16T00:00:00Z")
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

        let turns = try parseTurnUsage(
            #"[{"at_ms":1760000000000,"session":"ab12cd34","session_display":"Continue Codex thread task","provider_id":"baidu","catalog_slug":"glm-5.2","upstream_model_id":"glm-5.2","returned_model":"glm-4.6","model_matches":false,"protocol":"chat_completions","prefix_state":"append_only","input_tokens":2000,"cache_read_tokens":8000,"cache_creation_tokens":null,"output_tokens":300,"reasoning_tokens":null,"reasoning_chars":1500,"cache_hit_percent":40.0,"ttft_ms":812.5,"output_tps":null}]"#
        )
        precondition(turns[0].modelMatches == false && turns[0].returnedModel == "glm-4.6")
        precondition(turns[0].cacheIsLow, "a 10k-token prompt with 40% hits is flagged")
        precondition(turns[0].outputTPS == nil, "unknown speed stays nil")
        precondition(turns[0].sessionDisplay == "Continue Codex thread task")
        let daily = try parseDailyUsage(
            #"[{"day":20381,"provider_id":"a","model_id":"m","request_count":2,"input_tokens":10,"cache_read_tokens":30,"cache_creation_tokens":0,"output_tokens":5,"cache_hit_percent":75},{"day":20381,"provider_id":"b","model_id":"m","request_count":1,"input_tokens":1,"cache_read_tokens":0,"cache_creation_tokens":0,"output_tokens":1,"cache_hit_percent":0}]"#
        )
        precondition(UsageTotals(daily).requests == 3)
        precondition(UsageTotals(daily).cacheHitPercent == 30.0 / 41.0 * 100)
        let excluded = try parseDailyUsage(
            #"[{"day":20381,"provider_id":"renamed","model_id":"Opus-5","request_count":2,"input_tokens":32000,"cache_read_tokens":0,"cache_creation_tokens":0,"output_tokens":5,"cache_hit_percent":null}]"#
        )
        precondition(UsageTotals(daily + excluded).cacheHitPercent == UsageTotals(daily).cacheHitPercent)
        precondition(UsageTotals(daily + excluded).requests == 5)
        precondition(UsageTotals(excluded).cacheHitPercent == nil)
        let merged = usageChartPoints(daily, split: false, aggregateLabel: "all", lastDay: 20382, days: 3)
        precondition(merged.map(\.day) == [20380, 20381, 20382], "empty days stay on the axis")
        precondition(merged[1].tokens == 47 && merged[1].requests == 3 && merged[0].tokens == 0)
        let split = usageChartPoints(daily, split: true, aggregateLabel: "all", lastDay: 20381, days: 1)
        precondition(split.map(\.series) == ["a", "b"])
        precondition(split[0].dayLabel == "10/20")
        print("Console usage parsing: passed")
    }
}
