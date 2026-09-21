import Cocoa

private let lightApplicationIconName = "CodexMixin"
private let darkApplicationIconName = "CodexMixinDark"

func applicationIconResourceName(for appearance: NSAppearance) -> String {
    if appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua {
        return darkApplicationIconName
    }
    return lightApplicationIconName
}

@MainActor
final class ApplicationIconController {
    private var appearanceObservation: NSKeyValueObservation?

    func start(application: NSApplication) {
        appearanceObservation = application.observe(
            \.effectiveAppearance,
            options: [.initial, .new]
        ) { [weak self, weak application] _, _ in
            DispatchQueue.main.async { [weak self, weak application] in
                guard let self, let application else { return }
                self.updateIcon(for: application)
            }
        }
    }

    private func updateIcon(for application: NSApplication) {
        let resourceName = applicationIconResourceName(for: application.effectiveAppearance)
        guard let url = Bundle.main.url(forResource: resourceName, withExtension: "icns"),
              let image = NSImage(contentsOf: url)
        else {
            fatalError("Missing application icon resource: \(resourceName).icns")
        }
        application.applicationIconImage = image
    }
}
