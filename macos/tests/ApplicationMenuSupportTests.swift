import Cocoa

private final class QuitTarget: NSObject {
    @objc func quit(_ sender: Any?) {}
}

private final class CloseWindowTarget: NSObject {
    private let window: NSWindow?

    init(window: NSWindow? = nil) {
        self.window = window
    }

    @objc func closeCurrentWindow(_ sender: Any?) {
        closeWindow(window, sender: sender)
    }
}

private final class RecordingWindow: NSWindow {
    var didPerformClose = false

    override func performClose(_ sender: Any?) {
        didPerformClose = true
    }
}

@main
struct ApplicationMenuSupportTests {
    static func main() {
        let app = NSApplication.shared
        let recordingWindow = RecordingWindow(
            contentRect: .zero,
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        let closeWindowTarget = CloseWindowTarget(window: recordingWindow)
        let menu = makeApplicationMainMenu(
            quitTarget: QuitTarget(),
            quitAction: #selector(QuitTarget.quit(_:)),
            closeWindowTarget: closeWindowTarget,
            closeWindowAction: #selector(CloseWindowTarget.closeCurrentWindow(_:))
        )
        app.mainMenu = menu

        guard let windowMenu = menu.items.compactMap(\.submenu).first(where: {
            $0.title == L10n.Menu.window
        }), let closeItem = windowMenu.item(withTitle: L10n.Menu.closeWindow)
        else {
            preconditionFailure("The application menu must include Window > Close Window")
        }
        precondition(closeItem.keyEquivalent == "w")
        precondition(closeItem.keyEquivalentModifierMask == [.command])
        precondition(closeItem.target === closeWindowTarget)
        precondition(closeItem.action == #selector(CloseWindowTarget.closeCurrentWindow(_:)))

        guard let commandW = NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [.command],
            timestamp: 0,
            windowNumber: 0,
            context: nil,
            characters: "w",
            charactersIgnoringModifiers: "w",
            isARepeat: false,
            keyCode: 13
        ) else {
            preconditionFailure("The test must create a Command-W key event")
        }
        precondition(
            menu.performKeyEquivalent(with: commandW),
            "Command-W must invoke the close-window menu action"
        )
        precondition(
            recordingWindow.didPerformClose,
            "The close command must invoke performClose on the selected window"
        )

        let persistentWindow = NSPanel(
            contentRect: .zero,
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        persistentWindow.isReleasedWhenClosed = true
        persistentWindow.hidesOnDeactivate = true
        configurePersistentWindow(persistentWindow)
        precondition(
            persistentWindow.styleMask.contains(.closable),
            "Every app window must expose a close button"
        )
        precondition(
            !persistentWindow.isReleasedWhenClosed,
            "App windows must remain retained until explicitly closed"
        )
        precondition(
            !persistentWindow.hidesOnDeactivate,
            "App windows must remain visible when another app becomes active"
        )
        presentPersistentWindow(persistentWindow)
        precondition(
            app.activationPolicy() == .regular,
            "Visible app windows must make Codex Mixin available in Command-Tab"
        )
        persistentWindow.close()
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        precondition(
            app.activationPolicy() == .accessory,
            "Closing the final app window must restore menu bar-only mode"
        )
        print("Application menu close-window command: passed")
    }
}
