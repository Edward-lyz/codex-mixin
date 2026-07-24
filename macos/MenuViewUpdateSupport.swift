import Cocoa

final class MenuItemViewUpdater: NSObject, NSMenuDelegate {
    private struct PendingUpdate {
        let item: NSMenuItem
        let view: NSView
    }

    private var isMenuTracking = false
    private var pendingUpdates: [ObjectIdentifier: PendingUpdate] = [:]

    func setView(for item: NSMenuItem, build: @escaping () -> NSView) {
        guard Thread.isMainThread else {
            DispatchQueue.main.async { [weak self, weak item] in
                guard let self, let item else { return }
                self.setViewOnMain(build(), for: item)
            }
            return
        }
        setViewOnMain(build(), for: item)
    }

    private func setViewOnMain(_ view: NSView, for item: NSMenuItem) {
        guard isMenuTracking else {
            item.view = view
            return
        }
        pendingUpdates[ObjectIdentifier(item)] = PendingUpdate(item: item, view: view)
    }

    func menuWillOpen(_ menu: NSMenu) {
        isMenuTracking = true
    }

    func menuDidClose(_ menu: NSMenu) {
        isMenuTracking = false
        let updates = Array(pendingUpdates.values)
        pendingUpdates.removeAll()
        for update in updates {
            update.item.view = update.view
        }
    }
}
