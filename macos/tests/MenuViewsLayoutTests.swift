import Cocoa

enum GatewayError: Error {
    case command(String)
}

@main
struct MenuViewsLayoutTests {
    static func main() throws {
        UserDefaults.standard.removeObject(forKey: QuotaDisplayStyle.defaultsKey)
        _ = NSApplication.shared
        let percentage = try parseProviderQuotaUsage(#"[{"provider_id":"agy","unit":"percent","remaining":42,"used":58,"limit":100}]"#)[0]
        precondition(providerQuotaText(percentage) == "58 / 100", "Percentage quotas must show used allowance against the limit")
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
        precondition(
            gatewayStatusDetail(
                title: "官方 Codex 模式",
                endpoint: nil,
                statusDetail: nil,
                isRunning: false,
                isBusy: false
            ) == "GPT 直连官方 · Mixin 上游配置已保留"
        )

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
        flushDeferredMenuLayout()
        dashboard.layoutSubtreeIfNeeded()
        precondition(dashboard.frame.width == 336)
        let menuManagedWidth = dashboard.frame.width + 48
        dashboard.frame.size.width = menuManagedWidth
        let collapsedHeight = dashboard.frame.height
        precondition(collapsedHeight < 334)
        precondition(dashboard.model.groups.count == 5)
        precondition(
            !dashboard.model.groups.contains { $0.providerID == "disabled-provider" }
        )
        // The tab strip names the provider, so the menu has no provider header
        // and no token section: a one-window provider stays compact.
        precondition(collapsedHeight <= 200, "quota-only dashboard should stay compact, got \(collapsedHeight)")
        dashboard.model.selectProvider("opencode-go")
        precondition(
            dashboard.frame.height == collapsedHeight,
            "row resize must be deferred off the menu display cycle, not applied synchronously"
        )
        precondition(
            dashboard.frame.width == menuManagedWidth,
            "requesting a row-height sync must not change the menu-managed width synchronously"
        )
        flushDeferredMenuLayout()
        precondition(
            dashboard.frame.width == menuManagedWidth,
            "row-height sync must preserve the width managed by the menu"
        )

        let resetDashboard = ProviderUsageDashboardView()
        resetDashboard.updateConfiguredProviders([
            ProviderDashboardProvider(id: "agy", displayName: "Google Antigravity (agy)"),
        ])
        let resetRows = (1...3).map {
            #"{"provider_id":"agy","quota_id":"window.\#($0)","label":"Claude and GPT models - Weekly Limit Remaining","unit":"percent","remaining":97.3,"used":2.7,"limit":100,"reset_at":"2026-10-22T00:00:00Z"}"#
        }.joined(separator: ",")
        resetDashboard.updateQuotaUsages(try parseProviderQuotaUsage("[\(resetRows)]"))
        flushDeferredMenuLayout()
        precondition(resetDashboard.model.quotaStyle == .ring, "Rings are the default quota style")
        let ringHeight = resetDashboard.frame.height
        resetDashboard.model.selectQuotaStyle(.bar)
        flushDeferredMenuLayout()
        precondition(UserDefaults.standard.string(forKey: QuotaDisplayStyle.defaultsKey) == "bar")
        precondition(resetDashboard.frame.height >= 250, "Quota labels, bars and reset times need separate vertical space")
        precondition(ringHeight < resetDashboard.frame.height, "Rings share one row for all windows and need less height than bars")
        let ratioRows = try parseProviderQuotaUsage(#"[{"provider_id":"b","used":9,"limit":12},{"provider_id":"b","remaining":3,"limit":12},{"provider_id":"b","remaining":3}]"#)
        precondition(providerQuotaRatio(ratioRows[0]) == 0.75, "used / limit drives the ring")
        precondition(providerQuotaRatio(ratioRows[1]) == 0.75, "remaining derives usage when used is absent")
        precondition(providerQuotaRatio(ratioRows[2]) == nil, "balances without a limit draw no ring")

        if let artifactDirectory = ProcessInfo.processInfo.environment["CODEX_MIXIN_LAYOUT_ARTIFACT_DIR"] {
            let window = NSWindow(contentRect: resetDashboard.frame, styleMask: .borderless, backing: .buffered, defer: false)
            window.contentView = resetDashboard
            window.setFrameOrigin(NSPoint(x: -10000, y: -10000))
            window.orderFront(nil)
            for (name, appearance) in [("light", NSAppearance.Name.aqua), ("dark", NSAppearance.Name.darkAqua)] {
                resetDashboard.appearance = NSAppearance(named: appearance)
                resetDashboard.layoutSubtreeIfNeeded()
                window.displayIfNeeded()
                RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.2))
                guard let bitmap = resetDashboard.bitmapImageRepForCachingDisplay(in: resetDashboard.bounds) else { preconditionFailure("Dashboard bitmap unavailable") }
                resetDashboard.cacheDisplay(in: resetDashboard.bounds, to: bitmap)
                guard let png = bitmap.representation(using: .png, properties: [:]) else { preconditionFailure("Dashboard PNG unavailable") }
                try FileManager.default.createDirectory(atPath: artifactDirectory, withIntermediateDirectories: true)
                try png.write(to: URL(fileURLWithPath: artifactDirectory).appendingPathComponent("quota-\(name).png"))
            }
            window.orderOut(nil)
        }

        let proDashboard = ProviderUsageDashboardView()
        proDashboard.updateConfiguredProviders([
            ProviderDashboardProvider(id: "official", displayName: "OpenAI"),
        ])
        proDashboard.updateQuotaUsages(try parseProviderQuotaUsage(
            """
            [
              {"provider_id":"official","quota_id":"codex.secondary","label":"Codex · 7d","used":34,"limit":100},
              {"provider_id":"official","quota_id":"spark.primary","label":"GPT-5.3-Codex-Spark · 5h","used":0,"limit":100},
              {"provider_id":"official","quota_id":"spark.secondary","label":"GPT-5.3-Codex-Spark · 7d","used":0,"limit":100}
            ]
            """
        ))
        flushDeferredMenuLayout()
        proDashboard.layoutSubtreeIfNeeded()
        let threeQuotaScrollCount = descendants(of: proDashboard, matching: NSScrollView.self).count
        let threeQuotaHeight = proDashboard.frame.height
        let manyQuotaRows = (1...8).map {
            #"{"provider_id":"official","quota_id":"quota.\#($0)","label":"Quota \#($0)","used":\#($0),"limit":100}"#
        }.joined(separator: ",")
        proDashboard.updateQuotaUsages(try parseProviderQuotaUsage("[\(manyQuotaRows)]"))
        flushDeferredMenuLayout()
        proDashboard.layoutSubtreeIfNeeded()
        precondition(proDashboard.model.selectedGroup?.quotas.count == 8)
        precondition(
            proDashboard.frame.height == threeQuotaHeight,
            "quota sections with more than three rows must scroll instead of growing the menu"
        )
        precondition(
            descendants(of: proDashboard, matching: NSScrollView.self).count
                == threeQuotaScrollCount + 1,
            "quota overflow must add a vertical scroll view"
        )

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
            title: "本地服务运行中 · 1 个服务商需要处理",
            endpoint: "http://127.0.0.1:8787/v1",
            statusDetail: providerIssue,
            isRunning: true,
            isBusy: false,
            target: nil,
            action: #selector(NSApplication.terminate(_:))
        )
        guard let hostedServiceView = serviceView as? ServiceMenuHostingView else {
            preconditionFailure("Service menu must use its SwiftUI host")
        }
        precondition(hostedServiceView.model.detail == providerIssue)
        precondition(hostedServiceView.model.statusDetail == providerIssue)
        writeDashboardSnapshots(dashboard)
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

@MainActor
private func writeDashboardSnapshots(_ dashboard: ProviderUsageDashboardView) {
    guard let directory = ProcessInfo.processInfo.environment["MENU_DASHBOARD_SNAPSHOT_DIR"] else {
        return
    }
    let outputURL = URL(fileURLWithPath: directory, isDirectory: true)
    do {
        try FileManager.default.createDirectory(at: outputURL, withIntermediateDirectories: true)
    } catch {
        preconditionFailure("menu snapshot directory could not be created: \(error)")
    }

    flushDeferredMenuLayout()
    let window = NSWindow(
        contentRect: NSRect(origin: .zero, size: NSSize(width: 336, height: dashboard.frame.height)),
        styleMask: [.borderless],
        backing: .buffered,
        defer: false
    )
    window.contentView = dashboard
    window.isOpaque = true
    window.backgroundColor = .windowBackgroundColor
    window.orderFrontRegardless()
    dashboard.model.selectProvider("baidu-oneapi")

    for (name, appearanceName) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
        window.appearance = NSAppearance(named: appearanceName)
        flushDeferredMenuLayout()
        dashboard.layoutSubtreeIfNeeded()
        writeViewSnapshot(dashboard, to: outputURL.appendingPathComponent("menu-\(name).png"))
    }
    window.orderOut(nil)
}

private func writeViewSnapshot(_ view: NSView, to url: URL) {
    guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
        preconditionFailure("menu snapshot bitmap must be available")
    }
    view.cacheDisplay(in: view.bounds, to: bitmap)
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
        preconditionFailure("menu snapshot must encode as PNG")
    }
    do {
        try data.write(to: url)
    } catch {
        preconditionFailure("menu snapshot could not be written: \(error)")
    }
}

private func descendants<T: NSView>(of view: NSView, matching type: T.Type) -> [T] {
    view.subviews.flatMap { child in
        (child as? T).map { [$0] } ?? descendants(of: child, matching: type)
    }
}

/// Drains pending main run-loop work so a deferred dashboard row resize is
/// applied before the test inspects the view frame. The dashboard schedules
/// its resize with `DispatchQueue.main.async` to avoid mutating geometry while
/// the status-bar menu is committing its display cycle.
private func flushDeferredMenuLayout() {
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
}
