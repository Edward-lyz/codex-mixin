import AppKit
import SwiftUI

extension View {
    /// Keeps the primary action visually prominent, falling back to
    /// `.borderedProminent` when Reduce Transparency is enabled.
    func liquidGlassProminentButton() -> some View {
        modifier(LiquidGlassProminentButtonModifier())
    }
}

private struct LiquidGlassProminentButtonModifier: ViewModifier {
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    @ViewBuilder
    func body(content: Content) -> some View {
#if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            if reduceTransparency {
                content.buttonStyle(.borderedProminent)
            } else {
                content.buttonStyle(.glassProminent)
            }
        } else {
            content.buttonStyle(.borderedProminent)
        }
#else
        content.buttonStyle(.borderedProminent)
#endif
    }
}

/// Keeps a window fully opaque on the standard semantic background with a
/// normal title bar instead of a transparent Liquid Glass one.
func configureOpaqueWindow(_ window: NSWindow) {
    window.isOpaque = true
    window.backgroundColor = .windowBackgroundColor
    window.titlebarAppearsTransparent = false
    window.titlebarSeparatorStyle = .automatic
}
