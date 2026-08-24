import SwiftUI

extension View {
    /// Applies the system Liquid Glass material to a card or control.
    @ViewBuilder
    func liquidGlass<S: Shape>(in shape: S, interactive: Bool = false) -> some View {
        if #available(macOS 26.0, *) {
            glassEffect(
                interactive ? .regular.interactive() : .regular,
                in: shape
            )
        } else {
            background(.ultraThinMaterial, in: shape)
                .overlay(shape.stroke(.separator.opacity(0.45), lineWidth: 0.5))
        }
    }

    /// Extends a window's background underneath the transparent title bar.
    @ViewBuilder
    func liquidGlassWindowBackground() -> some View {
        if #available(macOS 26.0, *) {
            background(.ultraThinMaterial, ignoresSafeAreaEdges: .all)
                .backgroundExtensionEffect()
        } else {
            background(.ultraThinMaterial, ignoresSafeAreaEdges: .all)
        }
    }

    /// Keeps the primary action visually prominent before Liquid Glass is available.
    @ViewBuilder
    func liquidGlassProminentButton() -> some View {
        if #available(macOS 26.0, *) {
            buttonStyle(.glassProminent)
        } else {
            buttonStyle(.borderedProminent)
        }
    }
}
