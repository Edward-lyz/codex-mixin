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

        precondition(descendantViews(of: NSTextField.self, in: contentView)
            .allSatisfy { $0.placeholderString != "/v1/responses" && $0.placeholderString != "/v1/models" })
        print("Provider settings navigation: passed")
    }

    private static func descendantViews<T: NSView>(of type: T.Type, in root: NSView?) -> [T] {
        guard let root else { return [] }
        let current = (root as? T).map { [$0] } ?? []
        return current + root.subviews.flatMap { descendantViews(of: type, in: $0) }
    }
}
