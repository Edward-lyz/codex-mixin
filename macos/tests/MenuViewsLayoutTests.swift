import Cocoa

enum GatewayError: Error {
    case command(String)
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
        precondition(runningToggle.isOn)
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
        precondition(!busyToggle.isOn)
        precondition(!busyToggle.isEnabled)

        precondition(updateServiceMenuView(
            runningToggleView,
            title: "本地网关停止中...",
            endpoint: nil,
            statusDetail: nil,
            isRunning: false,
            isBusy: true
        ))
        precondition(!runningToggle.isOn)
        precondition(runningToggle.isBusy)
        precondition(!runningToggle.isEnabled)

        precondition(updateServiceMenuView(
            runningToggleView,
            title: "本地网关运行中",
            endpoint: "http://127.0.0.1:8787",
            statusDetail: nil,
            isRunning: true,
            isBusy: false
        ))
        precondition(runningToggle.isOn)
        precondition(!runningToggle.isBusy)
        precondition(runningToggle.isEnabled)

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
        let tokenUsages = try parseProviderTokenUsage(
            """
            [
              {
                "provider_id": "baidu-oneapi",
                "model_id": "gpt-5.6-sol",
                "request_count": 6,
                "input_tokens": 1500,
                "cache_read_tokens": 4500,
                "cache_creation_tokens": 500,
                "output_tokens": 300,
                "cache_hit_percent": 75.0
              },
              {
                "provider_id": "baidu-oneapi",
                "model_id": "DeepSeek-V4-Flash",
                "request_count": 3,
                "input_tokens": 200,
                "cache_read_tokens": 600,
                "cache_creation_tokens": 100,
                "output_tokens": 80,
                "cache_hit_percent": 66.7
              },
              {
                "provider_id": "custom-2",
                "model_id": "other-model",
                "request_count": 1,
                "input_tokens": 10,
                "cache_read_tokens": 0,
                "cache_creation_tokens": 0,
                "output_tokens": 5,
                "cache_hit_percent": null
              }
            ]
            """
        )
        let dashboard = ProviderUsageDashboardView()
        dashboard.updateQuotaUsages(usages)
        dashboard.updateTokenUsages(tokenUsages)
        dashboard.layoutSubtreeIfNeeded()
        precondition(dashboard.frame.width == 392)
        precondition(dashboard.frame.height == 354)

        let providerScroll = descendants(of: dashboard, matching: NSScrollView.self)
            .first { $0.identifier?.rawValue == "provider-tab-scroll" }
        precondition(providerScroll != nil)
        precondition(providerScroll?.hasVerticalScroller == false)

        let tokenLabels = descendants(of: dashboard, matching: NSTextField.self)
        precondition(tokenLabels.contains {
            $0.stringValue.contains("gpt-5.6-sol")
        })
        precondition(tokenLabels.contains {
            $0.stringValue.contains("DeepSeek-V4-Flash")
        })
        precondition(tokenLabels.contains {
            $0.stringValue == "缓存输出"
        })
        precondition(!tokenLabels.contains { $0.stringValue.contains("other-model") })
        let modelRows = descendants(of: dashboard, matching: NSControl.self)
        precondition(modelRows.contains {
            $0.toolTip?.contains("输入 1.5k") == true
                && $0.toolTip?.contains("缓存输入 4.5k") == true
                && $0.toolTip?.contains("输出 300") == true
                && $0.toolTip?.contains("缓存输出 500") == true
                && $0.toolTip?.contains("整体缓存比例 75.0%") == true
        })

        let customProviderButton = descendants(of: dashboard, matching: NSButton.self)
            .first { $0.identifier?.rawValue == "provider-tab-custom-2" }
        precondition(customProviderButton != nil)
        customProviderButton?.performClick(nil)
        dashboard.layoutSubtreeIfNeeded()
        precondition(descendants(of: dashboard, matching: NSTextField.self)
            .contains { $0.stringValue.contains("other-model") })
        precondition(!descendants(of: dashboard, matching: NSTextField.self)
            .contains { $0.stringValue.contains("gpt-5.6-sol") })
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
        print("Provider dashboard layout and switching: passed")
    }

    static func requireSwitch(in view: NSView) throws -> GatewaySwitchControl {
        if let toggle = view as? GatewaySwitchControl { return toggle }
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
