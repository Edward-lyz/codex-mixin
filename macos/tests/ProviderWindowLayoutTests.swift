import Cocoa

@main
struct ProviderWindowLayoutTests {
    static func main() {
        _ = NSApplication.shared

        let largeScreen = NSRect(x: 0, y: 0, width: 1440, height: 900)
        let largeSize = providerSettingsContentSize(for: largeScreen)
        precondition(largeSize.width == 800)
        precondition(largeSize.height == 550)
        precondition(largeSize.width <= largeScreen.width - 48)
        precondition(largeSize.height <= largeScreen.height - 48)

        let smallScreen = NSRect(x: 0, y: 0, width: 1_024, height: 700)
        let smallSize = providerSettingsContentSize(for: smallScreen)
        precondition(smallSize.width <= smallScreen.width - 32)
        precondition(smallSize.height <= smallScreen.height - 32)

        let combinedSize = modelBenchmarkContentSize(for: largeScreen)
        precondition(combinedSize.width == 1_180)
        precondition(combinedSize.height == 660)

        let modelScroll = NSScrollView()
        configureModelTableScrollView(modelScroll)
        precondition(modelScroll.hasVerticalScroller)
        precondition(modelScroll.hasHorizontalScroller)
        precondition(!modelScroll.autohidesScrollers)

        let benchmarkScroll = NSScrollView()
        configureModelTableScrollView(benchmarkScroll)
        precondition(benchmarkScroll.hasHorizontalScroller)

        print("Provider and benchmark window layout: passed")
    }
}
