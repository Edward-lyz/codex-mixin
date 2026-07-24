import Cocoa

enum GatewayError: Error {
    case command(String)
}

func appText(_ simplifiedChinese: String, _ traditionalChinese: String, _ english: String) -> String {
    simplifiedChinese
}

@main
struct MenuViewsLayoutTests {
    static func main() throws {
        _ = NSApplication.shared
        let usages = try parseProviderQuotaUsage(
            """
            [
              {
                "provider_id": "baidu-oneapi",
                "display_name": "Baidu OneAPI",
                "currency": "CNY",
                "used": 929.1,
                "limit": 1500,
                "remaining": 570.9
              },
              {
                "provider_id": "custom-2",
                "display_name": "AIHub",
                "currency": "USD",
                "used": 2.21,
                "limit": 12.01,
                "remaining": 9.8
              }
            ]
            """
        )
        let quotaView = providerQuotaMenuView(usages)
        quotaView.layoutSubtreeIfNeeded()
        let progressIndicators = descendants(
            of: quotaView,
            matching: NSProgressIndicator.self
        )

        precondition(progressIndicators.count == 2)
        let widths = progressIndicators.map(\.frame.width)
        precondition(
            abs(widths[0] - widths[1]) < 0.5,
            "All Provider quota tracks must have equal widths; got \(widths)"
        )
        print("Provider quota track widths: passed")
    }
}

private func descendants<T: NSView>(of view: NSView, matching type: T.Type) -> [T] {
    view.subviews.flatMap { child in
        (child as? T).map { [$0] } ?? descendants(of: child, matching: type)
    }
}
