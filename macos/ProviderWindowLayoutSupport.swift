import Cocoa

func providerSettingsContentSize(for visibleFrame: NSRect) -> NSSize {
    adaptiveWindowContentSize(
        for: visibleFrame,
        ideal: NSSize(width: 980, height: 700),
        minimum: NSSize(width: 860, height: 600)
    )
}

func providerSettingsSurface(
    material: NSVisualEffectView.Material = .contentBackground
) -> NSVisualEffectView {
    let surface = NSVisualEffectView()
    surface.material = material
    surface.blendingMode = .withinWindow
    surface.state = .active
    surface.wantsLayer = true
    surface.layer?.cornerRadius = 12
    surface.layer?.masksToBounds = true
    surface.translatesAutoresizingMaskIntoConstraints = false
    surface.setContentHuggingPriority(.defaultLow, for: .vertical)
    return surface
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
