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
            $0.title == "窗口"
        }), let closeItem = windowMenu.item(withTitle: "关闭窗口")
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
        print("Application menu close-window command: passed")
    }
}
