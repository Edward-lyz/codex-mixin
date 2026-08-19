import Cocoa

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    nil
}

@main
struct ProviderSettingsNavigationTests {
    static func main() {
        _ = NSApplication.shared

        testReloadPreservesSelectionAndDisablesEmptyDetails()
        testMutationIsBusyAndRestartsInOrder()
        testBaiduBridgeTestAndUpdateArguments()

        print("Provider settings behavior: passed")
    }

    private static func testReloadPreservesSelectionAndDisablesEmptyDetails() {
        var responses = [
            try! providerList(ids: ["custom", "baidu-oneapi"]),
            try! providerList(ids: ["baidu-oneapi", "custom"]),
            try! providerList(ids: []),
        ]
        var loadCalls = 0
        let controller = ProviderSettingsWindowController(
            loadHandler: {
                let response = responses.removeFirst()
                loadCalls += 1
                return response
            },
            runHandler: { _ in "" },
            applyHandler: { _ in }
        )
        guard let contentView = controller.window?.contentView else {
            preconditionFailure("Provider settings must build a content view")
        }
        precondition(descendantViews(of: NSTextField.self, in: contentView)
            .allSatisfy { $0.placeholderString != "/v1/responses" && $0.placeholderString != "/v1/models" })
        controller.present()

        let table = providerTable(in: controller)
        waitUntil {
            loadCalls == 1 && controller.numberOfRows(in: table) == 2
        }
        guard let contentView = controller.window?.contentView,
              let body = descendantViews(of: NSSplitView.self, in: contentView).first
        else {
            preconditionFailure("Provider settings split view is missing")
        }
        controller.window?.layoutIfNeeded()
        contentView.layoutSubtreeIfNeeded()
        precondition(
            body.frame.height > contentView.bounds.height * 0.6,
            "Provider settings body was compressed to the bottom of the window"
        )
        precondition(table.selectedRow == 0)

        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        waitUntil { table.selectedRow == 1 }
        precondition(providerID(in: controller, row: table.selectedRow) == "baidu-oneapi")

        controller.present()
        waitUntil {
            loadCalls == 2
                && controller.numberOfRows(in: table) == 2
                && table.selectedRow == 0
        }
        precondition(providerID(in: controller, row: table.selectedRow) == "baidu-oneapi")

        controller.present()
        waitUntil {
            loadCalls == 3 && controller.numberOfRows(in: table) == 0
        }
        for title in ["停用", "测试连接", "保存更改"] {
            precondition(button(title: title, in: controller)?.isEnabled == false)
        }
    }

    private static func testMutationIsBusyAndRestartsInOrder() {
        var loadCalls = 0
        var events: [String] = []
        let controller = ProviderSettingsWindowController(
            loadHandler: {
                loadCalls += 1
                events.append("reload")
                return try providerList(ids: ["custom"])
            },
            runHandler: { arguments in
                events.append("run:\(arguments.joined(separator: "|"))")
                try await Task.sleep(nanoseconds: 150_000_000)
                return ""
            },
            applyHandler: { _ in
                events.append("apply")
            }
        )
        controller.present()
        let table = providerTable(in: controller)
        waitUntil { loadCalls == 1 && controller.numberOfRows(in: table) == 1 }

        let toggle = button(title: "停用", in: controller)
        precondition(toggle?.isEnabled == true)
        let alertTimer = dismissModalAlerts()
        toggle?.performClick(nil)
        waitUntil {
            events.contains { $0.hasPrefix("run:") }
        }
        precondition(button(title: "新增", in: controller)?.isEnabled == false)
        precondition(button(title: "保存更改", in: controller)?.isEnabled == false)
        toggle?.performClick(nil)

        waitUntil {
            events.contains("apply") && loadCalls == 2 && NSApp.modalWindow == nil
        }
        alertTimer.invalidate()
        let runEvents = events.filter { $0.hasPrefix("run:") }
        precondition(runEvents == ["run:providers|disable|custom"])
        guard let runIndex = events.firstIndex(where: { $0.hasPrefix("run:") }),
              let applyIndex = events.firstIndex(of: "apply"),
              let reloadIndex = events.lastIndex(of: "reload")
        else {
            preconditionFailure("mutation events are incomplete")
        }
        precondition(runIndex < applyIndex && applyIndex < reloadIndex)
        precondition(button(title: "新增", in: controller)?.isEnabled == true)
    }

    private static func testBaiduBridgeTestAndUpdateArguments() {
        var events: [String] = []
        var setupModes: [BaiduAuthBridgeMode] = []
        var loadCalls = 0
        let executable = URL(fileURLWithPath: "/tmp/test-ducx")
        let controller = ProviderSettingsWindowController(
            loadHandler: {
                loadCalls += 1
                events.append("reload")
                return try providerList(ids: ["baidu-oneapi"])
            },
            runHandler: { arguments in
                events.append("run:\(arguments.joined(separator: "|"))")
                if arguments.contains("test") {
                    return """
                    {
                      "provider_id": "baidu-oneapi",
                      "ok": true,
                      "mode": "configuration",
                      "model_count": 1,
                      "paid_inference_performed": false
                    }
                    """
                }
                return ""
            },
            applyHandler: { _ in
                events.append("apply")
            },
            baiduBridgeSetupHandler: { mode in
                setupModes.append(mode)
                events.append("setup:\(mode.rawValue)")
                return executable
            }
        )
        controller.present()
        let table = providerTable(in: controller)
        waitUntil { loadCalls == 1 && controller.numberOfRows(in: table) == 1 }

        guard let popup = descendantViews(of: NSPopUpButton.self, in: controller.window?.contentView).first,
              let test = button(title: "测试连接", in: controller),
              let save = button(title: "保存更改", in: controller)
        else {
            preconditionFailure("Baidu provider controls are missing")
        }
        popup.selectItem(withTitle: "DUCX 核心（loopback）")
        NSApp.sendAction(popup.action!, to: popup.target, from: popup)
        precondition((popup.selectedItem?.representedObject as? String) == "ducx_loopback")

        let alertTimer = dismissModalAlerts()
        test.performClick(nil)
        waitUntil {
            events.contains {
                $0 == "run:providers|test|baidu-oneapi|--json|--baidu-auth-bridge|ducx_loopback|--ducx-executable|/tmp/test-ducx"
            }
        }
        precondition(setupModes == [.ducxLoopback])
        waitUntil { test.isEnabled && NSApp.modalWindow == nil }

        save.performClick(nil)
        waitUntil {
            events.contains {
                $0 == "run:providers|update|baidu-oneapi|--auxiliary-model-upstream|false|--quota-username|quota-user|--baidu-auth-bridge|ducx_loopback|--baidu-code-report|false|--ducx-executable|/tmp/test-ducx"
            }
        }
        waitUntil {
            events.filter { $0 == "apply" }.count == 1 && NSApp.modalWindow == nil
        }
        alertTimer.invalidate()

        precondition(setupModes == [.ducxLoopback, .ducxLoopback])
        let testRunIndex = events.firstIndex {
            $0 == "run:providers|test|baidu-oneapi|--json|--baidu-auth-bridge|ducx_loopback|--ducx-executable|/tmp/test-ducx"
        }
        let updateRunIndex = events.firstIndex {
            $0 == "run:providers|update|baidu-oneapi|--auxiliary-model-upstream|false|--quota-username|quota-user|--baidu-auth-bridge|ducx_loopback|--baidu-code-report|false|--ducx-executable|/tmp/test-ducx"
        }
        precondition(testRunIndex != nil && updateRunIndex != nil)
        precondition(events.firstIndex(of: "setup:ducx_loopback")! < testRunIndex!)
        precondition(events.lastIndex(of: "setup:ducx_loopback")! < updateRunIndex!)
    }

    private static func providerList(ids: [String]) throws -> ProviderListResponse {
        let providers = ids.map(providerJSON).joined(separator: ",")
        let json = """
            {
              "config_version": 1,
              "gateway_auth_configured": false,
              "providers": [\(providers)]
            }
            """
        return try decodeProviderList(json)
    }

    private static func providerJSON(id: String) -> String {
        let preset = id == "baidu-oneapi" ? "baidu-oneapi" : "custom"
        let protocolID = id == "baidu-oneapi" ? "anthropic_messages" : "open_ai_chat"
        let apiPath = id == "baidu-oneapi" ? "/v1/messages" : "/v1/chat/completions"
        let modelSourceKind = id == "baidu-oneapi" ? "baidu_oneapi" : "open_ai_compatible"
        let bridge = id == "baidu-oneapi"
            ? ", \"baidu_auth_bridge\": \"disabled\", \"baidu_code_report\": false, \"quota_username\": \"quota-user\""
            : ""
        return """
        {
          "id": "\(id)",
          "display_name": "\(id)",
          "enabled": true,
          "auxiliary_model_upstream": false,
          "preset_id": "\(preset)",
          "protocol": "\(protocolID)",
          "base_url": "https://example.com",
          "api_path": "\(apiPath)",
          "model_source": {"kind": "\(modelSourceKind)", "path": "/v1/models"},
          "api_key_configured": true,
          "quota_parser": "generic"\(bridge),
          "selected_models": [],
          "new_models": [],
          "unavailable_selected_models": [],
          "cached_models": [{"id": "model-1"}],
          "readiness": "healthy",
          "readiness_issues": [],
          "routable_model_count": 1
        }
        """
    }

    private static func providerTable(in controller: ProviderSettingsWindowController) -> NSTableView {
        guard let table = descendantViews(
            of: NSTableView.self,
            in: controller.window?.contentView
        ).first else {
            preconditionFailure("Provider table is missing")
        }
        return table
    }

    private static func providerID(
        in controller: ProviderSettingsWindowController,
        row: Int
    ) -> String? {
        guard let table = descendantViews(
            of: NSTableView.self,
            in: controller.window?.contentView
        ).first,
        let column = table.tableColumns.first,
        let cell = controller.tableView(table, viewFor: column, row: row) as? NSTableCellView
        else {
            return nil
        }
        return cell.textField?.stringValue
    }

    private static func button(title: String, in controller: ProviderSettingsWindowController) -> NSButton? {
        descendantViews(of: NSButton.self, in: controller.window?.contentView)
            .first { $0.title == title }
    }

    private static func dismissModalAlerts() -> Timer {
        let timer = Timer(timeInterval: 0.02, repeats: true) { _ in
            guard let modalContent = NSApp.modalWindow?.contentView,
                  let button = descendantViews(of: NSButton.self, in: modalContent).first
            else {
                return
            }
            button.performClick(nil)
        }
        RunLoop.main.add(timer, forMode: .common)
        return timer
    }

    private static func waitUntil(
        timeout: TimeInterval = 8,
        _ predicate: () -> Bool
    ) {
        let deadline = Date().addingTimeInterval(timeout)
        while !predicate() && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.02))
        }
        precondition(predicate(), "Timed out waiting for Provider settings state")
    }

    private static func descendantViews<T: NSView>(of type: T.Type, in root: NSView?) -> [T] {
        guard let root else { return [] }
        let current = (root as? T).map { [$0] } ?? []
        return current + root.subviews.flatMap { descendantViews(of: type, in: $0) }
    }
}
