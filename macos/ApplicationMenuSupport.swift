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
    let appMenu = NSMenu(title: "Codex Mixin")
    let quitItem = appMenu.addItem(withTitle: "退出", action: quitAction, keyEquivalent: "q")
    quitItem.target = quitTarget
    appMenuItem.submenu = appMenu
    mainMenu.addItem(appMenuItem)

    let editMenuItem = NSMenuItem()
    let editMenu = NSMenu(title: "编辑")
    editMenu.addItem(withTitle: "剪切", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
    editMenu.addItem(withTitle: "复制", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    editMenu.addItem(withTitle: "粘贴", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    editMenu.addItem(withTitle: "全选", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
    editMenuItem.submenu = editMenu
    mainMenu.addItem(editMenuItem)

    let windowMenuItem = NSMenuItem()
    let windowMenu = NSMenu(title: "窗口")
    let closeWindowItem = windowMenu.addItem(
        withTitle: "关闭窗口",
        action: closeWindowAction,
        keyEquivalent: "w"
    )
    closeWindowItem.keyEquivalentModifierMask = [.command]
    closeWindowItem.target = closeWindowTarget
    windowMenuItem.submenu = windowMenu
    mainMenu.addItem(windowMenuItem)

    return mainMenu
}
