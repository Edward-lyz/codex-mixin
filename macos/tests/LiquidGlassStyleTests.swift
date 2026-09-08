import AppKit

@main
struct LiquidGlassStyleTests {
    static func main() {
        _ = NSApplication.shared
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 320, height: 180),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        window.isOpaque = false
        window.backgroundColor = .clear
        window.titlebarAppearsTransparent = true
        window.titlebarSeparatorStyle = .none
        configureOpaqueWindow(window)
        precondition(window.isOpaque, "window must remain opaque")
        precondition(
            window.backgroundColor == .windowBackgroundColor,
            "window must use the semantic window background"
        )
        precondition(!window.titlebarAppearsTransparent, "title bar must remain opaque")
        precondition(
            window.titlebarSeparatorStyle == .automatic,
            "title bar must keep the native separator"
        )
        print("Opaque window contract: passed")
    }
}
