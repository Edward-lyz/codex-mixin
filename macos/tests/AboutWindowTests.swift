import Cocoa

func appText(_ simplifiedChinese: String, _ traditionalChinese: String, _ english: String) -> String {
    simplifiedChinese
}

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    nil
}

@main
struct AboutWindowTests {
    static func main() {
        _ = NSApplication.shared

        let info = AppAboutInfo(
            version: "0.3.8",
            build: "0.3.8",
            repositoryURL: "https://github.com/Edward-lyz/codex-mixin"
        )
        let controller = AboutWindowController(info: info)
        controller.present()
        guard let window = controller.window else {
            preconditionFailure("About window must exist")
        }
        precondition(window.title == "关于 Codex Mixin")
        precondition(
            window.contentView != nil,
            "About window must have content"
        )
        window.contentView?.layoutSubtreeIfNeeded()

        let labels = descendantViews(of: NSTextField.self, in: window.contentView)
        precondition(labels.contains { $0.stringValue == "Codex Mixin" })
        precondition(labels.contains { $0.stringValue == "版本 0.3.8" })
        precondition(labels.contains { $0.stringValue == "Build 0.3.8" })

        let buttons = descendantViews(of: NSButton.self, in: window.contentView)
        precondition(buttons.contains { $0.title == "GitHub 仓库" })
        precondition(buttons.contains { $0.title == "复制版本信息" })
        precondition(buttons.contains { $0.toolTip == info.repositoryURL })
        print("About window layout: passed")
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
