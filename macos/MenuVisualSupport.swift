import Cocoa

class FlippedMenuView: NSView {
    override var isFlipped: Bool { true }
}

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    guard #available(macOS 11.0, *) else {
        return nil
    }
    guard let image = NSImage(systemSymbolName: systemSymbolName, accessibilityDescription: nil) else {
        return nil
    }
    image.isTemplate = true
    return image
}

func codexStatusImage(isRunning: Bool) -> NSImage {
    let size = NSSize(width: 22, height: 22)
    let image = NSImage(size: size)
    image.lockFocus()

    let bounds = NSRect(origin: .zero, size: size)
    NSColor.clear.setFill()
    bounds.fill()

    let shadow = NSShadow()
    shadow.shadowOffset = NSSize(width: 0, height: -0.6)
    shadow.shadowBlurRadius = 1.6
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.22)
    shadow.set()

    let body = NSBezierPath(
        roundedRect: NSRect(x: 2.2, y: 2.0, width: 17.8, height: 17.8),
        xRadius: 6.0,
        yRadius: 6.0
    )
    let startColor = NSColor(calibratedRed: 0.20, green: 0.53, blue: 1.00, alpha: 1.0)
    let endColor = NSColor(calibratedRed: 0.54, green: 0.32, blue: 0.98, alpha: 1.0)
    NSGradient(starting: startColor, ending: endColor)?.draw(in: body, angle: 35)

    let glow = NSBezierPath(ovalIn: NSRect(x: 3.7, y: 9.8, width: 15.2, height: 8.0))
    NSColor.white.withAlphaComponent(0.20).setFill()
    glow.fill()

    let prompt = NSBezierPath()
    prompt.lineWidth = 1.9
    prompt.lineCapStyle = .round
    prompt.lineJoinStyle = .round
    prompt.move(to: NSPoint(x: 7.2, y: 8.0))
    prompt.line(to: NSPoint(x: 10.2, y: 11.0))
    prompt.line(to: NSPoint(x: 7.2, y: 14.0))
    NSColor.white.withAlphaComponent(0.95).setStroke()
    prompt.stroke()

    let cursor = NSBezierPath()
    cursor.lineWidth = 1.9
    cursor.lineCapStyle = .round
    cursor.move(to: NSPoint(x: 12.4, y: 8.2))
    cursor.line(to: NSPoint(x: 15.8, y: 8.2))
    cursor.stroke()

    let statusRing = NSBezierPath(ovalIn: NSRect(x: 14.3, y: 2.0, width: 7.2, height: 7.2))
    NSColor.white.withAlphaComponent(0.88).setFill()
    statusRing.fill()

    let statusDot = NSBezierPath(ovalIn: NSRect(x: 15.1, y: 2.8, width: 5.6, height: 5.6))
    (isRunning ? NSColor.systemGreen : NSColor.systemOrange).setFill()
    statusDot.fill()

    image.unlockFocus()
    image.isTemplate = false
    return image
}
