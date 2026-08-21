import Cocoa

enum GatewayError: Error {
    case command(String)
}

@main
struct MenuViewsLayoutTests {
    static func main() throws {
        _ = NSApplication.shared
        guard let providerWebsiteURL = URL(string: "https://example.com/settings") else {
            preconditionFailure("test provider website URL is invalid")
        }
        let faviconURL = declaredFaviconURL(
            html: #"<link rel="icon" href="/assets/provider.png">"#,
            baseURL: providerWebsiteURL
        )
        precondition(faviconURL?.absoluteString == "https://example.com/assets/provider.png")
        let embeddedFaviconURL = declaredFaviconURL(
            html: #"<link rel="icon" href="data:image/png;base64,iVBORw0KGgo=">"#,
            baseURL: providerWebsiteURL
        )
        precondition(embeddedProviderIconData(from: embeddedFaviconURL)?.count == 8)
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
                "provider_display_name": "Baidu OneAPI",
                "display_name": "Baidu OneAPI",
                "quota_id": "quota",
                "label": "Quota",
                "currency": "CNY",
                "used": 929.1,
                "limit": 1500,
                "remaining": 570.9
              },
              {
                "provider_id": "custom-2",
                "provider_display_name": "AIHub",
                "display_name": "AIHub",
                "quota_id": "quota",
                "label": "Quota",
                "currency": "USD",
                "used": 2.21,
                "limit": 12.01,
                "remaining": 9.8
              },
              {
                "provider_id": "deepseek",
                "provider_display_name": "DeepSeek",
                "display_name": "DeepSeek",
                "quota_id": "balance",
                "label": "Balance",
                "currency": "CNY",
                "used": null,
                "remaining": 110
              },
              {
                "provider_id": "opencode-go",
                "provider_display_name": "OpenCode Go",
                "display_name": "OpenCode Go 5h",
                "quota_id": "five_hour",
                "label": "5h",
                "used": 7,
                "limit": 100,
                "remaining": 93
              },
              {
                "provider_id": "opencode-go",
                "provider_display_name": "OpenCode Go",
                "display_name": "OpenCode Go Weekly",
                "quota_id": "weekly",
                "label": "Weekly",
                "used": 12,
                "limit": 100,
                "remaining": 88
              },
              {
                "provider_id": "opencode-go",
                "provider_display_name": "OpenCode Go",
                "display_name": "OpenCode Go Monthly",
                "quota_id": "monthly",
                "label": "Monthly",
                "used": 31,
                "limit": 100,
                "remaining": 69
              },
              {
                "provider_id": "opencode-go",
                "provider_display_name": "OpenCode Go",
                "display_name": "OpenCode Go Balance",
                "quota_id": "balance",
                "label": "Balance",
                "currency": "USD",
                "used": null,
                "remaining": 42.5
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
        let disabledQuota = try parseProviderQuotaUsage(
            """
            [{
              "provider_id": "disabled-provider",
              "provider_display_name": "Disabled Provider",
              "quota_id": "quota",
              "used": 90,
              "limit": 100,
              "remaining": 10
            }]
            """
        )
        let disabledTokenUsage = try parseProviderTokenUsage(
            """
            [{
              "provider_id": "disabled-provider",
              "model_id": "stale-model",
              "request_count": 9,
              "input_tokens": 900,
              "cache_read_tokens": 0,
              "cache_creation_tokens": 0,
              "output_tokens": 90,
              "cache_hit_percent": null
            }]
            """
        )
        let dashboard = ProviderUsageDashboardView()
        dashboard.updateConfiguredProviders([
            ProviderDashboardProvider(id: "baidu-oneapi", displayName: "Baidu OneAPI"),
            ProviderDashboardProvider(id: "opencode-go", displayName: "OpenCode Go"),
            ProviderDashboardProvider(id: "deepseek", displayName: "DeepSeek"),
            ProviderDashboardProvider(id: "custom-2", displayName: "AIHub"),
            ProviderDashboardProvider(id: "idle-provider", displayName: "Idle Provider"),
            ProviderDashboardProvider(
                id: "disabled-provider",
                displayName: "Disabled Provider",
                isEnabled: false
            ),
        ])
        dashboard.updateQuotaUsages(usages + disabledQuota)
        dashboard.updateTokenUsages(tokenUsages + disabledTokenUsage)
        dashboard.layoutSubtreeIfNeeded()
        precondition(dashboard.frame.width == 336)
        let collapsedHeight = dashboard.frame.height
        precondition(collapsedHeight < 334)
        precondition(dashboard.model.groups.count == 5)
        precondition(
            !dashboard.model.groups.contains { $0.providerID == "disabled-provider" }
        )
        precondition(dashboard.model.selectedGroup?.models.count == 2)
        precondition(dashboard.model.selectedGroup?.models.first?.modelID == "gpt-5.6-sol")
        dashboard.model.selectModel("gpt-5.6-sol")
        precondition(dashboard.frame.height > collapsedHeight)
        dashboard.model.selectModel("gpt-5.6-sol")
        precondition(dashboard.frame.height == collapsedHeight)

        let scrollingDashboard = ProviderUsageDashboardView()
        scrollingDashboard.updateConfiguredProviders([
            ProviderDashboardProvider(id: "baidu-oneapi", displayName: "Baidu OneAPI"),
        ])
        scrollingDashboard.updateTokenUsages([
            ProviderTokenUsage(providerID: "baidu-oneapi", modelID: "model-1", requestCount: 1, inputTokens: 400, cacheReadTokens: 0, cacheCreationTokens: 0, outputTokens: 0, cacheHitPercent: nil),
            ProviderTokenUsage(providerID: "baidu-oneapi", modelID: "model-2", requestCount: 1, inputTokens: 300, cacheReadTokens: 0, cacheCreationTokens: 0, outputTokens: 0, cacheHitPercent: nil),
            ProviderTokenUsage(providerID: "baidu-oneapi", modelID: "model-3", requestCount: 1, inputTokens: 200, cacheReadTokens: 0, cacheCreationTokens: 0, outputTokens: 0, cacheHitPercent: nil),
            ProviderTokenUsage(providerID: "baidu-oneapi", modelID: "model-4", requestCount: 1, inputTokens: 100, cacheReadTokens: 0, cacheCreationTokens: 0, outputTokens: 0, cacheHitPercent: nil),
        ])
        precondition(scrollingDashboard.model.selectedGroup?.models.count == 4)
        scrollingDashboard.model.selectModel("model-4")
        precondition(scrollingDashboard.model.selectedModel?.modelID == "model-4")

        guard let primaryUsage = dashboard.model.selectedGroup?.models.first else {
            preconditionFailure("Baidu token usage must exist")
        }
        let usageDetail = tokenUsageDetail(primaryUsage)
        precondition(usageDetail.contains("输入 1.5k"))
        precondition(usageDetail.contains("缓存输入 4.5k"))
        precondition(usageDetail.contains("输出 300"))
        precondition(usageDetail.contains("缓存输出 500"))
        precondition(usageDetail.contains("整体缓存比例 75.0%"))

        dashboard.model.selectProvider("custom-2")
        precondition(dashboard.model.selectedGroup?.models.first?.modelID == "other-model")
        dashboard.model.selectProvider("opencode-go")
        guard let openCodeGroup = dashboard.model.selectedGroup else {
            preconditionFailure("OpenCode Go group must exist")
        }
        precondition(openCodeGroup.displayName == "OpenCode Go")
        precondition(openCodeGroup.quotas.count == 4)
        let quotaLabels = openCodeGroup.quotas.map {
            providerQuotaLabel($0, multiple: true)
        }
        precondition(quotaLabels == ["5h", "1 周", "月度", "余额"])
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
