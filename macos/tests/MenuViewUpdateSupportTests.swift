import Cocoa

@main
struct MenuViewUpdateSupportTests {
    static func main() {
        let updater = MenuItemViewUpdater()
        let menu = NSMenu()
        let item = NSMenuItem()
        let originalView = NSView()
        let supersededView = NSView()
        let latestView = NSView()

        item.view = originalView
        menu.addItem(item)
        updater.menuWillOpen(menu)
        updater.setView(for: item) { supersededView }
        updater.setView(for: item) { latestView }

        precondition(
            item.view === originalView,
            "The menu item view must remain stable while its menu is tracking"
        )

        updater.menuDidClose(menu)
        precondition(
            item.view === latestView,
            "The latest deferred view must be installed after menu tracking ends"
        )

        let backgroundView = NSView()
        let queued = DispatchSemaphore(value: 0)
        DispatchQueue.global().async {
            updater.setView(for: item) { backgroundView }
            queued.signal()
        }
        precondition(queued.wait(timeout: .now() + 1) == .success)
        precondition(
            item.view === latestView,
            "A background request must not mutate AppKit before reaching the main queue"
        )
        RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
        precondition(
            item.view === backgroundView,
            "A background request must be applied after reaching the main queue"
        )
        print("Menu view update deferral: passed")
    }
}
