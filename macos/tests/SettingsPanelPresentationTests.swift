import Cocoa

func appText(_ simplifiedChinese: String, _ traditionalChinese: String, _ english: String) -> String {
    simplifiedChinese
}

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    nil
}

func showAlert(title: String, message: String) {
    preconditionFailure("The presentation test must not display an alert")
}

@main
struct SettingsPanelPresentationTests {
    static func main() {
        _ = NSApplication.shared
        let parent = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 800, height: 600),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        var resultWasDelivered = false

        runAddProviderSheet(attachedTo: parent) { values in
            precondition(values == nil)
            resultWasDelivered = true
        }

        guard let sheet = parent.attachedSheet else {
            preconditionFailure("The add-provider form must be attached to its settings window")
        }
        precondition(!(sheet is NSPanel), "The add-provider form must be a regular NSWindow")
        precondition(sheet.level == .normal, "The add-provider sheet must use the normal window level")
        precondition(NSApp.modalWindow == nil, "The add-provider form must not start an app-modal loop")
        let popups = descendantViews(of: NSPopUpButton.self, in: sheet.contentView)
        guard let bridgePopup = popups.first(where: {
            $0.itemTitles.contains("DUCC 核心（loopback）")
        }) else {
            preconditionFailure("The Baidu setup must offer the DUCC auth bridge")
        }
        precondition(
            bridgePopup.numberOfItems == 2,
            "The auth bridge selector must contain only disabled and DUCC"
        )
        precondition(
            bridgePopup.selectedItem?.representedObject as? String == "disabled",
            "Auth bridging must be disabled by default"
        )

        parent.endSheet(sheet, returnCode: .cancel)
        RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
        precondition(resultWasDelivered)
        print("Add-provider sheet presentation: passed")
    }

    private static func descendantViews<T: NSView>(
        of type: T.Type,
        in root: NSView?
    ) -> [T] {
        guard let root else { return [] }
        let current = (root as? T).map { [$0] } ?? []
        return current + root.subviews.flatMap { descendantViews(of: type, in: $0) }
    }
}
