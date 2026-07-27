import Cocoa

func providerSettingsContentSize(for visibleFrame: NSRect) -> NSSize {
    adaptiveWindowContentSize(
        for: visibleFrame,
        ideal: NSSize(width: 800, height: 430),
        minimum: NSSize(width: 720, height: 400)
    )
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
