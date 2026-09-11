import Cocoa

@main
struct ProviderWindowLayoutTests {
    static func main() {
        _ = NSApplication.shared

        let largeScreen = NSRect(x: 0, y: 0, width: 1440, height: 900)
        let largeSize = providerSettingsContentSize(for: largeScreen)
        precondition(largeSize.width == 1_180)
        precondition(largeSize.height == 720)
        precondition(largeSize.width <= largeScreen.width - 48)
        precondition(largeSize.height <= largeScreen.height - 48)

        let smallScreen = NSRect(x: 0, y: 0, width: 1_024, height: 700)
        let smallSize = providerSettingsContentSize(for: smallScreen)
        precondition(smallSize.width <= smallScreen.width - 32)
        precondition(smallSize.height <= smallScreen.height - 32)
        precondition(providerSidebarMinimumWidth == 220)
        precondition(providerSidebarIdealWidth == 250)
        precondition(providerSidebarMaximumWidth == 300)
        precondition(providerSidebarMinimumWidth < providerSidebarIdealWidth)
        precondition(providerSidebarIdealWidth < providerSidebarMaximumWidth)
        precondition(modelTableMinimumWidth(includesRatio: false) == 1_100)
        precondition(modelTableMinimumWidth(includesRatio: true) == 1_190)
        precondition(
            modelTableMinimumWidth(includesRatio: true)
                > modelTableMinimumWidth(includesRatio: false)
        )

        print("Models and services window layout: passed")
    }
}
