import Cocoa

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    nil
}

@main
struct ProviderSettingsNavigationTests {
    static func main() {
        _ = NSApplication.shared
        let controller = ProviderSettingsWindowController(
            loadHandler: {
                try decodeProviderList(
                    """
                    {
                      "config_version": 1,
                      "gateway_auth_configured": false,
                      "providers": []
                    }
                    """
                )
            },
            runHandler: { _ in "" },
            applyHandler: { _ in }
        )
        guard let contentView = controller.window?.contentView else {
            preconditionFailure("Provider settings must build a content view")
        }

        let popups = descendantViews(of: NSPopUpButton.self, in: contentView)
        guard let protocolPopup = popups.first(where: {
            $0.itemTitles.contains("OpenAI Responses")
        }) else {
            preconditionFailure("Custom provider protocol selector is missing")
        }
        precondition(protocolPopup.itemTitles.count == 3)
        precondition(
            protocolPopup.itemArray.compactMap { $0.representedObject as? String }
                == ["open_ai_responses", "anthropic_messages", "open_ai_chat"]
        )

        let fields = descendantViews(of: NSTextField.self, in: contentView)
        precondition(fields.contains { $0.placeholderString == "/v1/responses" })
        precondition(fields.contains { $0.placeholderString == "/v1/models" })
        precondition(
            customProviderEndpointArguments(
                protocolID: "open_ai_responses",
                apiPath: "/v1/responses",
                modelsPath: "/v1/models"
            ) == [
                "--protocol", "open_ai_responses",
                "--api-path", "/v1/responses",
                "--models-path", "/v1/models",
            ]
        )
        precondition(
            customProviderEndpointArguments(
                protocolID: "open_ai_responses",
                apiPath: "",
                modelsPath: "/v1/models"
            ) == nil
        )
        print("Provider settings endpoint navigation: passed")
    }

    private static func descendantViews<T: NSView>(of type: T.Type, in root: NSView?) -> [T] {
        guard let root else { return [] }
        let current = (root as? T).map { [$0] } ?? []
        return current + root.subviews.flatMap { descendantViews(of: type, in: $0) }
    }
}
