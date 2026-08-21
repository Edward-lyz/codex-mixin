import Cocoa
import SwiftUI

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
        precondition(
            AppLocalization.string("installCard.codexMixinCardDayWithMixin", 42)
                == "Codex Mixin card, day 42 with Mixin"
        )
        let identity = CardIdentityV1(
            installationID: UUID(uuidString: "59B26835-9033-4B89-B8CC-57ACB42C8C9B")!,
            firstRecordedAt: Date(timeIntervalSince1970: 1_722_470_400),
            seedVersion: 1
        )
        var openedURL: URL?
        var copiedText: String?
        var shownCardWallpaperOffset: Int?
        let controller = AboutWindowController(
            info: info,
            cardIdentity: identity,
            wallpaperOffset: 2,
            openURL: { openedURL = $0 },
            copyText: { copiedText = $0 },
            showCard: { shownCardWallpaperOffset = $0 }
        )
        controller.present()
        guard let window = controller.window else {
            preconditionFailure("About window must exist")
        }
        precondition(window.title == L10n.About.title)
        precondition(
            window.contentView != nil,
            "About window must have content"
        )
        window.contentView?.layoutSubtreeIfNeeded()
        if let snapshotPath = ProcessInfo.processInfo.environment["ABOUT_WINDOW_SNAPSHOT"] {
            writeSnapshot(of: window, to: snapshotPath)
        }
        controller.model.openRepository()
        controller.model.copyVersionInfo()
        precondition(openedURL?.absoluteString == info.repositoryURL)
        precondition(copiedText == info.versionSummary)
        precondition(controller.model.copied)

        controller.model.openCard()
        precondition(shownCardWallpaperOffset == 2)
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
}
