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
        let runningToggleView = serviceMenuView(
            title: "本地网关运行中",
            endpoint: "http://127.0.0.1:8787",
            statusDetail: nil,
            isRunning: true,
            isBusy: false,
            target: nil,
            action: #selector(NSApplication.terminate(_:))
        )
        let runningToggle = try requireSwitch(in: runningToggleView)
        precondition(runningToggleView.frame.height == 56)
        precondition(runningToggle.state == .on)
        precondition(runningToggle.isEnabled)

        let busyToggleView = serviceMenuView(
            title: "本地网关停止中...",
            endpoint: nil,
            statusDetail: nil,
            isRunning: false,
            isBusy: true,
            target: nil,
            action: #selector(NSApplication.terminate(_:))
        )
        let busyToggle = try requireSwitch(in: busyToggleView)
        precondition(busyToggle.state == .off)
        precondition(!busyToggle.isEnabled)

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
        let quotaLabels = descendants(of: quotaView, matching: NSTextField.self)
        precondition(quotaLabels.contains { $0.stringValue == "余额 110 CNY" })
        let providerIssue = "Baidu OneAPI：模型 unreachable-model 当前不可达"
        let serviceView = serviceMenuView(
            title: "本地网关运行中 · Provider 降级",
            endpoint: "http://127.0.0.1:8787/v1",
            statusDetail: providerIssue,
            isRunning: true,
            isBusy: false,
            target: nil,
            action: #selector(NSApplication.terminate(_:))
        )
        let labels = descendants(of: serviceView, matching: NSTextField.self)
        precondition(labels.contains { $0.stringValue == providerIssue })
        precondition(labels.contains { $0.toolTip == providerIssue })
        print("Provider quota track widths: passed")
    }

    static func requireSwitch(in view: NSView) throws -> NSSwitch {
        if let toggle = view as? NSSwitch { return toggle }
        for subview in view.subviews {
            if let toggle = try? requireSwitch(in: subview) { return toggle }
        }
        throw GatewayError.command("missing gateway toggle")
    }
}

private func descendants<T: NSView>(of view: NSView, matching type: T.Type) -> [T] {
    view.subviews.flatMap { child in
        (child as? T).map { [$0] } ?? descendants(of: child, matching: type)
    }
}
