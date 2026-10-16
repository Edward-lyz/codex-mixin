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

extension View {
    /// Draws a crisp edge under the toolbar instead of the macOS 26+ soft
    /// blur. Use it on a column whose top holds fixed (non-scrolling) content
    /// such as a header or picker; the soft blur would otherwise cover it.
    @ViewBuilder
    func fixedHeaderScrollEdge() -> some View {
#if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            scrollEdgeEffectStyle(.hard, for: .top)
        } else {
            self
        }
#else
        self
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
