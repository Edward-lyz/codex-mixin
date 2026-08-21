import SwiftUI

extension View {
    /// Applies the system Liquid Glass material to a card or control.
    func liquidGlass<S: Shape>(in shape: S, interactive: Bool = false) -> some View {
        glassEffect(
            interactive ? .regular.interactive() : .regular,
            in: shape
        )
    }

    /// Extends a window's background underneath the transparent title bar.
    func liquidGlassWindowBackground() -> some View {
        background(.ultraThinMaterial, ignoresSafeAreaEdges: .all)
            .backgroundExtensionEffect()
    }
}
