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
              }
            ]
            """
        )

        precondition(usages[0].menuLabel == "AIHub")
        precondition(usages[1].menuLabel == "custom-3")
        print("Provider quota labels: passed")
    }
}
