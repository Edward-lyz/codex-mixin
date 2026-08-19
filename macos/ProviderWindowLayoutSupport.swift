import Cocoa
import SwiftUI

func providerSettingsContentSize(for visibleFrame: NSRect) -> NSSize {
    adaptiveWindowContentSize(
        for: visibleFrame,
        ideal: NSSize(width: 980, height: 700),
        minimum: NSSize(width: 860, height: 600)
    )
}

func providerSettingsSurface(
    material: NSVisualEffectView.Material = .contentBackground
) -> NSView {
    let surface = ProviderSettingsSurfaceView(material: material)
    surface.wantsLayer = true
    surface.layer?.cornerRadius = 12
    surface.layer?.masksToBounds = true
    surface.translatesAutoresizingMaskIntoConstraints = false
    surface.setContentHuggingPriority(.defaultLow, for: .vertical)
    return surface
}

final class ProviderSettingsSurfaceView: NSView {
    private let backgroundView: NSView

    init(material: NSVisualEffectView.Material) {
        if #available(macOS 26.0, *) {
            backgroundView = NSHostingView(rootView: ProviderLiquidGlassBackground())
        } else {
            let visualEffectView = NSVisualEffectView()
            visualEffectView.material = material
            visualEffectView.blendingMode = .withinWindow
            visualEffectView.state = .active
            backgroundView = visualEffectView
        }
        super.init(frame: .zero)
        backgroundView.translatesAutoresizingMaskIntoConstraints = false
        addSubview(backgroundView, positioned: .below, relativeTo: nil)
        NSLayoutConstraint.activate([
            backgroundView.leadingAnchor.constraint(equalTo: leadingAnchor),
            backgroundView.trailingAnchor.constraint(equalTo: trailingAnchor),
            backgroundView.topAnchor.constraint(equalTo: topAnchor),
            backgroundView.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}

@available(macOS 26.0, *)
private struct ProviderLiquidGlassBackground: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 12, style: .continuous)
            .fill(.clear)
            .glassEffect()
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

func modelBenchmarkContentSize(for visibleFrame: NSRect) -> NSSize {
    adaptiveWindowContentSize(
        for: visibleFrame,
        ideal: NSSize(width: 1_180, height: 660),
        minimum: NSSize(width: 920, height: 520)
    )
}

func configureModelTableScrollView(_ scrollView: NSScrollView) {
    scrollView.hasVerticalScroller = true
    scrollView.hasHorizontalScroller = true
    scrollView.autohidesScrollers = false
    scrollView.borderType = .bezelBorder
}

private func adaptiveWindowContentSize(
    for visibleFrame: NSRect,
    ideal: NSSize,
    minimum: NSSize
) -> NSSize {
    let horizontalMargin: CGFloat = visibleFrame.width < 1_100 ? 32 : 48
    let verticalMargin: CGFloat = visibleFrame.height < 760 ? 32 : 48
    let availableWidth = max(visibleFrame.width - horizontalMargin, 640)
    let availableHeight = max(visibleFrame.height - verticalMargin, 440)
    return NSSize(
        width: min(ideal.width, max(minimum.width, availableWidth)),
        height: min(ideal.height, max(minimum.height, availableHeight))
    )
}
