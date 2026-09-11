import Cocoa

let providerSidebarMinimumWidth: CGFloat = 220
let providerSidebarIdealWidth: CGFloat = 250
let providerSidebarMaximumWidth: CGFloat = 300
let providerModelTableMinimumWidth: CGFloat = 1_100
let baiduProviderModelTableMinimumWidth: CGFloat = 1_190

func modelTableMinimumWidth(includesRatio: Bool) -> CGFloat {
    includesRatio ? baiduProviderModelTableMinimumWidth : providerModelTableMinimumWidth
}

func providerSettingsContentSize(for visibleFrame: NSRect) -> NSSize {
    adaptiveWindowContentSize(
        for: visibleFrame,
        ideal: NSSize(width: 1_180, height: 720),
        minimum: NSSize(width: 960, height: 600)
    )
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
