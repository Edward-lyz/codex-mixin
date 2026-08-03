import Cocoa

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
