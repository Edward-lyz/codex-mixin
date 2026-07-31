import Cocoa
import SwiftUI

func appText(_ simplifiedChinese: String, _ traditionalChinese: String, _ english: String) -> String {
    simplifiedChinese
}

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    nil
}

func showAlert(title: String, message: String) {
    preconditionFailure("Unexpected alert: \(title): \(message)")
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
        let identity = CardIdentityV1(
            installationID: UUID(uuidString: "59B26835-9033-4B89-B8CC-57ACB42C8C9B")!,
            firstRecordedAt: Date(timeIntervalSince1970: 1_722_470_400),
            seedVersion: 1
        )
        var openedURL: URL?
        var copiedText: String?
        var cardWasShown = false
        let controller = AboutWindowController(
            info: info,
            cardIdentity: identity,
            openURL: { openedURL = $0 },
            copyText: { copiedText = $0 },
            showCard: { cardWasShown = true }
        )
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
        precondition(buttons.contains { $0.title == "打开 GitHub 仓库" })
        precondition(buttons.contains { $0.title == "复制版本信息" })
        precondition(buttons.contains { $0.toolTip == info.repositoryURL })

        guard
            let repositoryButton = buttons.first(where: {
                $0.identifier?.rawValue == "about.repository"
            }),
            let copyButton = buttons.first(where: {
                $0.identifier?.rawValue == "about.copy-version"
            })
        else {
            preconditionFailure("About actions must be discoverable")
        }
        if let snapshotPath = ProcessInfo.processInfo.environment["ABOUT_WINDOW_SNAPSHOT"] {
            writeSnapshot(of: window, to: snapshotPath)
        }
        repositoryButton.performClick(nil)
        copyButton.performClick(nil)
        precondition(openedURL?.absoluteString == info.repositoryURL)
        precondition(copiedText == info.versionSummary)
        precondition(copyButton.title == "已复制")

        let cardPreviews = descendantViews(
            of: NSHostingView<InstallCardThumbnailView>.self,
            in: window.contentView
        )
        precondition(cardPreviews.count == 1)
        precondition(cardPreviews[0].identifier?.rawValue == "about.card-preview")
        cardPreviews[0].rootView.onOpen()
        precondition(cardWasShown)

        let brandPanel = descendantViews(of: NSVisualEffectView.self, in: window.contentView)
        precondition(brandPanel.count == 1)
        precondition(brandPanel[0].frame.width == 350)
        precondition(window.frame.size == NSSize(width: 820, height: 460))
        print("About window layout: passed")
    }

    private static func writeSnapshot(of window: NSWindow, to path: String) {
        guard
            let contentView = window.contentView,
            let bitmap = contentView.bitmapImageRepForCachingDisplay(in: contentView.bounds)
        else {
            preconditionFailure("About window snapshot bitmap must be available")
        }
        contentView.cacheDisplay(in: contentView.bounds, to: bitmap)
        guard let data = bitmap.representation(using: .png, properties: [:]) else {
            preconditionFailure("About window snapshot must encode as PNG")
        }
        do {
            try data.write(to: URL(fileURLWithPath: path))
        } catch {
            preconditionFailure("About window snapshot could not be written: \(error)")
        }
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
