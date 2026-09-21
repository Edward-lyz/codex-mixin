import Cocoa

@main
struct ApplicationIconSupportTests {
    private static let maximumPixelDifference = 10_000

    static func main() {
        let application = NSApplication.shared
        application.finishLaunching()
        guard let lightAppearance = NSAppearance(named: .aqua),
              let darkAppearance = NSAppearance(named: .darkAqua)
        else {
            fatalError("System Aqua appearances are unavailable")
        }

        precondition(applicationIconResourceName(for: lightAppearance) == "CodexMixin")
        precondition(applicationIconResourceName(for: darkAppearance) == "CodexMixinDark")

        application.appearance = lightAppearance
        let controller = ApplicationIconController()
        controller.start(application: application)
        waitUntilIcon(application, matches: "CodexMixin")

        application.appearance = darkAppearance
        waitUntilIcon(application, matches: "CodexMixinDark")
        waitUntilDockTile(application, matches: "CodexMixinDark")

        precondition(application.activationPolicy() == .accessory)
        precondition(application.setActivationPolicy(.regular))
        waitUntilIcon(application, matches: "CodexMixinDark")
        waitUntilDockTile(application, matches: "CodexMixinDark")
    }

    private static func waitUntilIcon(_ application: NSApplication, matches resourceName: String) {
        guard let expectedURL = Bundle.main.url(forResource: resourceName, withExtension: "icns"),
              let expectedImage = NSImage(contentsOf: expectedURL)
        else {
            fatalError("Missing test icon resource: \(resourceName).icns")
        }
        let deadline = Date().addingTimeInterval(2)
        let expectedPixels = renderedPixels(expectedImage)
        var actualPixels = application.applicationIconImage.map(renderedPixels)
        var difference = actualPixels.map { pixelDifference($0, expectedPixels) }
        while difference.map({ $0 > maximumPixelDifference }) != false, Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
            actualPixels = application.applicationIconImage.map(renderedPixels)
            difference = actualPixels.map { pixelDifference($0, expectedPixels) }
        }
        precondition(
            difference.map { $0 <= maximumPixelDifference } == true,
            "Application icon did not switch to \(resourceName)"
        )
    }

    private static func waitUntilDockTile(
        _ application: NSApplication,
        matches resourceName: String
    ) {
        guard let expectedURL = Bundle.main.url(forResource: resourceName, withExtension: "icns"),
              let expectedImage = NSImage(contentsOf: expectedURL)
        else {
            fatalError("Missing test icon resource: \(resourceName).icns")
        }
        let expectedPixels = renderedPixels(expectedImage)
        let deadline = Date().addingTimeInterval(2)
        var difference = dockTileDifference(application, expectedPixels: expectedPixels)
        while difference.map({ $0 > maximumPixelDifference }) != false, Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
            difference = dockTileDifference(application, expectedPixels: expectedPixels)
        }
        precondition(
            difference.map { $0 <= maximumPixelDifference } == true,
            "Dock tile did not switch to \(resourceName)"
        )
    }

    private static func dockTileDifference(
        _ application: NSApplication,
        expectedPixels: Data
    ) -> Int? {
        guard let imageView = application.dockTile.contentView as? NSImageView,
              let image = imageView.image
        else {
            return nil
        }
        return pixelDifference(renderedPixels(image), expectedPixels)
    }

    private static func pixelDifference(_ lhs: Data, _ rhs: Data) -> Int {
        guard lhs.count == rhs.count else { return .max }
        return zip(lhs, rhs).reduce(0) { difference, values in
            difference + abs(Int(values.0) - Int(values.1))
        }
    }

    private static func renderedPixels(_ image: NSImage) -> Data {
        guard let bitmap = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: 64,
            pixelsHigh: 64,
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        ), let context = NSGraphicsContext(bitmapImageRep: bitmap),
        let pixels = bitmap.bitmapData
        else {
            fatalError("Unable to render application icon")
        }

        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        image.draw(in: NSRect(x: 0, y: 0, width: 64, height: 64))
        context.flushGraphics()
        NSGraphicsContext.restoreGraphicsState()
        return Data(bytes: pixels, count: bitmap.bytesPerRow * bitmap.pixelsHigh)
    }
}
