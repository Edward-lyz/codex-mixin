import Cocoa

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    nil
}

@main
struct ProviderSettingsNavigationTests {
    @MainActor
    static func main() {
        _ = NSApplication.shared

        testReloadPreservesSelectionAndDisablesEmptyDetails()
        testMutationIsBusyAndRestartsInOrder()
        testBaiduBridgeTestAndUpdateArguments()
        testAWSBedrockEndpointArguments()
        testReorderReloadsPersistedProviderState()
        testOneAPIFieldsStayWithTheirProvider()

        print("Provider settings behavior: passed")
    }

    @MainActor
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
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))

        waitUntil {
            loadCalls == 1 && controller.model.providers.count == 2
        }
        precondition(controller.model.selectedProviderID == "custom")

        controller.model.selectProvider("baidu-oneapi")
        waitUntil { controller.model.selectedProviderID == "baidu-oneapi" }
        precondition(controller.model.selectedProvider?.id == "baidu-oneapi")

        controller.present()
        waitUntil {
            loadCalls == 2
                && controller.model.providers.count == 2
                && controller.model.selectedProviderID == "baidu-oneapi"
        }
        precondition(controller.model.selectedProvider?.id == "baidu-oneapi")

        controller.present()
        waitUntil {
            loadCalls == 3 && controller.model.providers.isEmpty
                && controller.model.selectedProviderID == nil
        }
        precondition(controller.model.canModifySelectedProvider == false)
    }

    @MainActor
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
        waitUntil {
            loadCalls == 1 && controller.model.providers.count == 1
                && controller.model.selectedProviderID == "custom"
        }

        precondition(controller.model.canModifySelectedProvider == true)
        let alertTimer = dismissModalAlerts()
        controller.toggleProvider()
        waitUntil {
            events.contains { $0.hasPrefix("run:") }
        }
        precondition(controller.model.canAddProvider == false)
        precondition(controller.model.canModifySelectedProvider == false)
        controller.toggleProvider()

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
        precondition(controller.model.canAddProvider == true)
    }

    @MainActor
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
        waitUntil {
            loadCalls == 1 && controller.model.providers.count == 1
                && controller.model.selectedProviderID == "baidu-oneapi"
        }

        controller.model.baiduAuthBridge = .ducxLoopback
        let alertTimer = dismissModalAlerts()
        controller.testProvider()
        waitUntil {
            events.contains {
                $0 == "run:providers|test|baidu-oneapi|--json|--baidu-auth-bridge|ducx_loopback|--ducx-executable|/tmp/test-ducx"
            }
        }
        precondition(setupModes == [.ducxLoopback])
        waitUntil { controller.model.canModifySelectedProvider && NSApp.modalWindow == nil }

        controller.saveProvider()
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

    @MainActor
    private static func testAWSBedrockEndpointArguments() {
        var events: [String] = []
        var loadCalls = 0
        let region = "eu-west-1"
        let controller = ProviderSettingsWindowController(
            loadHandler: {
                loadCalls += 1
                return try providerList(ids: ["aws-bedrock"])
            },
            runHandler: { arguments in
                events.append(arguments.joined(separator: "|"))
                if arguments.contains("test") {
                    return """
                    {
                      "provider_id": "aws-bedrock",
                      "ok": true,
                      "mode": "configuration",
                      "model_count": 3,
                      "paid_inference_performed": false
                    }
                    """
                }
                return ""
            },
            applyHandler: { _ in }
        )
        controller.present()
        waitUntil {
            loadCalls == 1 && controller.model.selectedProviderID == "aws-bedrock"
        }
        controller.model.awsRegion = region
        let alertTimer = dismissModalAlerts()

        controller.testProvider()
        waitUntil {
            events.contains(
                "providers|test|aws-bedrock|--json|--aws-region|\(region)"
            )
        }
        waitUntil { controller.model.canModifySelectedProvider && NSApp.modalWindow == nil }

        controller.saveProvider()
        waitUntil {
            events.contains(
                "providers|update|aws-bedrock|--auxiliary-model-upstream|false|--aws-region|\(region)"
            )
        }
        alertTimer.invalidate()
    }

    @MainActor
    private static func testReorderReloadsPersistedProviderState() {
        var loadCalls = 0
        var events: [String] = []
        let controller = ProviderSettingsWindowController(
            loadHandler: {
                loadCalls += 1
                return try providerList(ids: loadCalls == 1
                    ? ["baidu-oneapi", "baidu-oneapi-2"]
                    : ["baidu-oneapi-2", "baidu-oneapi"])
            },
            runHandler: { arguments in
                events.append(arguments.joined(separator: "|"))
                return ""
            },
            applyHandler: { _ in }
        )
        controller.present()
        waitUntil { loadCalls == 1 && controller.model.providers.count == 2 }

        controller.moveProviders(from: IndexSet(integer: 1), to: 0)

        waitUntil { loadCalls == 2 && controller.model.canModifySelectedProvider }
        precondition(events == ["providers|reorder|baidu-oneapi-2|baidu-oneapi"])
        precondition(controller.model.providers.map(\.id) == ["baidu-oneapi-2", "baidu-oneapi"])
        precondition(controller.model.selectedProviderID == "baidu-oneapi-2")
    }

    @MainActor
    private static func testOneAPIFieldsStayWithTheirProvider() {
        let controller = ProviderSettingsWindowController(
            loadHandler: {
                try providerList(ids: ["baidu-oneapi", "baidu-oneapi-2"])
            },
            runHandler: { _ in "" },
            applyHandler: { _ in }
        )
        controller.present()
        waitUntil { controller.model.providers.count == 2 }

        controller.model.selectProvider("baidu-oneapi-2")
        precondition(controller.model.quotaUsername == "quota-user-2")
        precondition(controller.model.baiduAuthBridge == .ducxLoopback)

        controller.model.selectProvider("baidu-oneapi")
        precondition(controller.model.quotaUsername == "quota-user")
        precondition(controller.model.baiduAuthBridge == .disabled)
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
        let isBaidu = id.hasPrefix("baidu-oneapi")
        let isAWSBedrock = id == "aws-bedrock"
        let preset = isBaidu ? "baidu-oneapi" : isAWSBedrock ? "aws-bedrock" : "custom"
        let protocolID = isBaidu || isAWSBedrock ? "anthropic_messages" : "open_ai_chat"
        let apiPath = isBaidu || isAWSBedrock ? "/v1/messages" : "/v1/chat/completions"
        let modelSource = isBaidu
            ? "{\"kind\": \"baidu_oneapi\", \"path\": \"/v1/models\"}"
            : isAWSBedrock
            ? "{\"kind\": \"static\"}"
            : "{\"kind\": \"open_ai_compatible\", \"path\": \"/v1/models\"}"
        let bridge = id == "baidu-oneapi"
            ? ", \"baidu_auth_bridge\": \"disabled\", \"baidu_code_report\": false, \"quota_username\": \"quota-user\""
            : id == "baidu-oneapi-2"
            ? ", \"baidu_auth_bridge\": \"ducx_loopback\", \"baidu_code_report\": true, \"quota_username\": \"quota-user-2\""
            : ""
        let aws = isAWSBedrock
            ? ", \"aws_sigv4_configured\": true, \"aws_region\": \"us-east-1\", \"aws_session_token_configured\": false"
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
          "model_source": \(modelSource),
          "api_key_configured": true\(aws),
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

    private static func swiftUIDescendantViews<T: NSView>(of type: T.Type, in root: NSView?) -> [T] {
        guard let root else { return [] }
        var result: [T] = []
        var queue: [NSView] = [root]
        while !queue.isEmpty {
            let current = queue.removeFirst()
            if let match = current as? T {
                result.append(match)
            }
            queue.append(contentsOf: current.subviews)
        }
        return result
    }
}
