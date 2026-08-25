import Cocoa

private final class PersistentWindowCoordinator: NSObject {
    static let shared = PersistentWindowCoordinator()

    private let windows = NSHashTable<NSWindow>.weakObjects()

    func configure(_ window: NSWindow) {
        window.styleMask.insert(.closable)
        window.isReleasedWhenClosed = false
        window.hidesOnDeactivate = false
        guard !windows.contains(window) else { return }
        windows.add(window)
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(windowWillClose(_:)),
            name: NSWindow.willCloseNotification,
            object: window
        )
        if let closeButton = window.standardWindowButton(.closeButton) {
            closeButton.target = self
            closeButton.action = #selector(closeWindow(_:))
        }
    }

    func present(_ window: NSWindow) {
        configure(window)
        NSApp.setActivationPolicy(.regular)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc private func closeWindow(_ sender: NSButton) {
        guard let window = sender.window else { return }
        if let parent = window.sheetParent {
            parent.endSheet(window, returnCode: .cancel)
            return
        }
        if NSApp.modalWindow === window {
            NSApp.abortModal()
        }
        window.close()
    }

    @objc private func windowWillClose(_ notification: Notification) {
        DispatchQueue.main.async { [weak self] in
            guard let self,
                  !windows.allObjects.contains(where: \.isVisible)
            else { return }
            NSApp.setActivationPolicy(.accessory)
        }
    }
}

func configurePersistentWindow(_ window: NSWindow) {
    PersistentWindowCoordinator.shared.configure(window)
}

func presentPersistentWindow(_ window: NSWindow) {
    PersistentWindowCoordinator.shared.present(window)
}

func closeWindow(_ window: NSWindow?, sender: Any?) {
    window?.performClose(sender)
}

func makeApplicationMainMenu(
    quitTarget: AnyObject,
    quitAction: Selector,
    closeWindowTarget: AnyObject,
    closeWindowAction: Selector
) -> NSMenu {
    let mainMenu = NSMenu()

    let appMenuItem = NSMenuItem()
    let appMenu = NSMenu(title: L10n.App.name)
    let quitItem = appMenu.addItem(withTitle: L10n.Menu.quit, action: quitAction, keyEquivalent: "q")
    quitItem.target = quitTarget
    appMenuItem.submenu = appMenu
    mainMenu.addItem(appMenuItem)

    let editMenuItem = NSMenuItem()
    let editMenu = NSMenu(title: L10n.Menu.edit)
    editMenu.addItem(withTitle: L10n.Menu.cut, action: #selector(NSText.cut(_:)), keyEquivalent: "x")
    editMenu.addItem(withTitle: L10n.Menu.copy, action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    editMenu.addItem(withTitle: L10n.Menu.paste, action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    editMenu.addItem(withTitle: L10n.Menu.selectAll, action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
    editMenuItem.submenu = editMenu
    mainMenu.addItem(editMenuItem)

    let windowMenuItem = NSMenuItem()
    let windowMenu = NSMenu(title: L10n.Menu.window)
    let closeWindowItem = windowMenu.addItem(
        withTitle: L10n.Menu.closeWindow,
        action: closeWindowAction,
        keyEquivalent: "w"
    )
    closeWindowItem.keyEquivalentModifierMask = [.command]
    closeWindowItem.target = closeWindowTarget
    windowMenuItem.submenu = windowMenu
    mainMenu.addItem(windowMenuItem)

    return mainMenu
}
