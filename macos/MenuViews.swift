import Cocoa

private let menuContentWidth: CGFloat = 336
private let serviceMenuHeight: CGFloat = 56
private let providerDashboardMinimumHeight: CGFloat = 168
private let providerTabWidth: CGFloat = 36
private let providerQuotaRowHeight: CGFloat = 42
private let visibleProviderQuotaRows = 3
private let tokenModelRowHeight: CGFloat = 30
private let visibleTokenModelRows = 3
private let gatewayStatusDotIdentifier = NSUserInterfaceItemIdentifier("gateway-status-dot")
private let gatewayTitleIdentifier = NSUserInterfaceItemIdentifier("gateway-title")
private let gatewayDetailIdentifier = NSUserInterfaceItemIdentifier("gateway-detail")
private let gatewaySwitchIdentifier = NSUserInterfaceItemIdentifier("gateway-switch")

final class GatewaySwitchControl: NSControl {
    private let trackLayer = CALayer()
    private let knobLayer = CALayer()
    private var busySpinner: CAShapeLayer?

    var isOn = false {
        didSet {
            guard isOn != oldValue else { return }
            animateSwitchTransition()
        }
    }
    var isBusy = false {
        didSet {
            guard isBusy != oldValue else { return }
            updateTrackColor(animated: true)
            if isBusy {
                showBusySpinner()
            } else {
                hideBusySpinner()
            }
        }
    }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        setupLayers()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        wantsLayer = true
        setupLayers()
    }

    private func setupLayers() {
        guard let layer else { return }
        trackLayer.cornerRadius = 13
        layer.addSublayer(trackLayer)

        knobLayer.cornerRadius = 12
        knobLayer.backgroundColor = NSColor.white.cgColor
        knobLayer.borderColor = NSColor.black.withAlphaComponent(0.16).cgColor
        knobLayer.borderWidth = 0.75
        knobLayer.shadowColor = NSColor.black.cgColor
        knobLayer.shadowOpacity = 0.12
        knobLayer.shadowRadius = 2
        knobLayer.shadowOffset = CGSize(width: 0, height: -1)
        layer.addSublayer(knobLayer)
    }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        layoutLayers()
        CATransaction.commit()
        updateTrackColor(animated: false)
    }

    private func layoutLayers() {
        let trackRect = bounds.insetBy(dx: 1, dy: 2)
        trackLayer.frame = trackRect

        let knobSize: CGFloat = 24
        let knobX = isOn ? bounds.maxX - knobSize - 3 : bounds.minX + 3
        knobLayer.frame = NSRect(
            x: knobX,
            y: (bounds.height - knobSize) / 2,
            width: knobSize,
            height: knobSize
        )

        if let spinner = busySpinner {
            let spinnerSize: CGFloat = 12
            spinner.frame = NSRect(
                x: knobLayer.frame.midX - spinnerSize / 2,
                y: knobLayer.frame.midY - spinnerSize / 2,
                width: spinnerSize,
                height: spinnerSize
            )
        }
    }

    private func currentTrackColor() -> NSColor {
        if isOn {
            return isBusy ? .systemGreen.withAlphaComponent(0.65) : .systemGreen
        }
        return .white.withAlphaComponent(isEnabled ? 0.96 : 0.7)
    }

    private func updateTrackColor(animated: Bool) {
        let color = currentTrackColor().cgColor
        let borderColor: CGColor? = isOn ? nil : NSColor.separatorColor.cgColor
        if animated {
            let anim = CABasicAnimation(keyPath: "backgroundColor")
            anim.fromValue = trackLayer.backgroundColor
            anim.toValue = color
            anim.duration = 0.22
            anim.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            trackLayer.add(anim, forKey: "trackColor")
        }
        trackLayer.backgroundColor = color
        trackLayer.borderColor = borderColor
        trackLayer.borderWidth = isOn ? 0 : 1
    }

    private func animateSwitchTransition() {
        let knobSize: CGFloat = 24
        let targetX = isOn ? bounds.maxX - knobSize - 3 : bounds.minX + 3
        let targetFrame = NSRect(
            x: targetX,
            y: (bounds.height - knobSize) / 2,
            width: knobSize,
            height: knobSize
        )

        let anim = CABasicAnimation(keyPath: "position")
        anim.fromValue = NSValue(point: NSPoint(x: knobLayer.frame.midX, y: knobLayer.frame.midY))
        anim.toValue = NSValue(point: NSPoint(x: targetFrame.midX, y: targetFrame.midY))
        anim.duration = 0.22
        anim.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        knobLayer.add(anim, forKey: "knobSlide")
        knobLayer.frame = targetFrame

        updateTrackColor(animated: true)
    }

    private func showBusySpinner() {
        guard busySpinner == nil else { return }
        let spinnerSize: CGFloat = 12
        let spinner = CAShapeLayer()
        let path = CGPath(
            ellipseIn: CGRect(x: 1, y: 1, width: spinnerSize - 2, height: spinnerSize - 2),
            transform: nil
        )
        spinner.path = path
        spinner.fillColor = nil
        spinner.strokeColor = NSColor.white.cgColor
        spinner.lineWidth = 1.5
        spinner.lineCap = .round
        spinner.strokeStart = 0
        spinner.strokeEnd = 0.75

        let rotation = CABasicAnimation(keyPath: "transform.rotation.z")
        rotation.fromValue = 0
        rotation.toValue = CGFloat.pi * 2
        rotation.duration = 0.8
        rotation.repeatCount = .infinity
        spinner.add(rotation, forKey: "spinnerRotation")

        knobLayer.addSublayer(spinner)
        busySpinner = spinner

        let spinnerFrame = NSRect(
            x: (knobLayer.bounds.width - spinnerSize) / 2,
            y: (knobLayer.bounds.height - spinnerSize) / 2,
            width: spinnerSize,
            height: spinnerSize
        )
        spinner.frame = spinnerFrame
    }

    private func hideBusySpinner() {
        busySpinner?.removeFromSuperlayer()
        busySpinner = nil
    }

    override func mouseDown(with event: NSEvent) {
        guard isEnabled, !isBusy else { return }
        isOn.toggle()
        sendAction(action, to: target)
    }
}

private func gatewayStatusColor(title: String, isRunning: Bool, isBusy: Bool) -> NSColor {
    if title.contains("失败") { return .systemRed }
    if title.contains("等待配置") || title.contains("降级") || title.contains("无启用") || isBusy {
        return .systemOrange
    }
    return isRunning ? .systemGreen : .systemGray
}

private func gatewayStatusDetail(
    title: String,
    endpoint: String?,
    statusDetail: String?,
    isRunning: Bool,
    isBusy: Bool
) -> String {
    if let statusDetail, !statusDetail.isEmpty { return statusDetail }
    if let endpoint { return endpoint }
    if title.contains("失败") { return "请查看运行日志" }
    if title.contains("等待配置") { return "请先设置供应商与 API Key" }
    if isBusy { return "正在切换本地网关" }
    return isRunning ? "正在读取本地接口地址" : "网关当前未运行"
}

private let statusDotPulseAnimationKey = "statusDotPulse"

private func animateLayerProperty(
    _ layer: CALayer,
    keyPath: String,
    toValue: Any,
    duration: CFTimeInterval = 0.3
) {
    let anim = CABasicAnimation(keyPath: keyPath)
    anim.fromValue = layer.value(forKeyPath: keyPath)
    anim.toValue = toValue
    anim.duration = duration
    anim.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
    layer.add(anim, forKey: keyPath)
    layer.setValue(toValue, forKeyPath: keyPath)
}

func updateServiceMenuView(
    _ view: NSView,
    title: String,
    endpoint: String?,
    statusDetail: String?,
    isRunning: Bool,
    isBusy: Bool
) -> Bool {
    guard
        let statusDot = descendant(in: view, identifier: gatewayStatusDotIdentifier),
        let titleLabel = descendant(in: view, identifier: gatewayTitleIdentifier) as? NSTextField,
        let detailLabel = descendant(in: view, identifier: gatewayDetailIdentifier) as? NSTextField,
        let toggle = descendant(in: view, identifier: gatewaySwitchIdentifier) as? GatewaySwitchControl
    else { return false }

    let statusColor = gatewayStatusColor(title: title, isRunning: isRunning, isBusy: isBusy)
    if let dotLayer = statusDot.layer {
        let isFailure = title.contains("失败")
        let isDegraded = title.contains("等待配置") || title.contains("降级") || title.contains("无启用") || isBusy
        let shouldPulse = isRunning && !isFailure && !isDegraded
            && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion

        animateLayerProperty(dotLayer, keyPath: "backgroundColor", toValue: statusColor.cgColor)
        animateLayerProperty(dotLayer, keyPath: "borderColor", toValue: statusColor.withAlphaComponent(0.28).cgColor)
        dotLayer.shadowColor = statusColor.cgColor

        dotLayer.removeAnimation(forKey: statusDotPulseAnimationKey)
        if shouldPulse {
            let pulse = CABasicAnimation(keyPath: "shadowOpacity")
            pulse.fromValue = 0.25
            pulse.toValue = 0.55
            pulse.duration = 1.6
            pulse.autoreverses = true
            pulse.repeatCount = .infinity
            pulse.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            dotLayer.add(pulse, forKey: statusDotPulseAnimationKey)
        } else {
            let targetShadow: Float = isRunning ? 0.45 : 0
            animateLayerProperty(dotLayer, keyPath: "shadowOpacity", toValue: targetShadow)
        }
    }
    titleLabel.stringValue = title
    detailLabel.stringValue = gatewayStatusDetail(
        title: title,
        endpoint: endpoint,
        statusDetail: statusDetail,
        isRunning: isRunning,
        isBusy: isBusy
    )
    detailLabel.toolTip = statusDetail
    toggle.isOn = isRunning
    toggle.isBusy = isBusy
    toggle.isEnabled = !isBusy
    return true
}

private func descendant(in view: NSView, identifier: NSUserInterfaceItemIdentifier) -> NSView? {
    if view.identifier == identifier { return view }
    for subview in view.subviews {
        if let match = descendant(in: subview, identifier: identifier) { return match }
    }
    return nil
}

func serviceMenuView(
    title: String,
    endpoint: String?,
    statusDetail: String?,
    isRunning: Bool,
    isBusy: Bool,
    target: AnyObject?,
    action: Selector
) -> NSView {
    let view = NSView(frame: NSRect(x: 0, y: 0, width: menuContentWidth, height: serviceMenuHeight))
    let statusColor = gatewayStatusColor(title: title, isRunning: isRunning, isBusy: isBusy)

    let statusDot = NSView()
    statusDot.wantsLayer = true
    statusDot.layer?.cornerRadius = 6
    statusDot.layer?.backgroundColor = statusColor.cgColor
    statusDot.layer?.borderWidth = 2
    statusDot.layer?.borderColor = statusColor.withAlphaComponent(0.28).cgColor
    statusDot.layer?.shadowColor = statusColor.cgColor
    statusDot.layer?.shadowOpacity = isRunning ? 0.45 : 0
    statusDot.layer?.shadowRadius = 3
    statusDot.translatesAutoresizingMaskIntoConstraints = false
    statusDot.identifier = gatewayStatusDotIdentifier

    let toggle = GatewaySwitchControl()
    toggle.isOn = isRunning
    toggle.isBusy = isBusy
    toggle.isEnabled = !isBusy
    toggle.target = target
    toggle.action = action
    toggle.translatesAutoresizingMaskIntoConstraints = false
    toggle.identifier = gatewaySwitchIdentifier

    let titleLabel = NSTextField(labelWithString: title)
    titleLabel.font = .systemFont(ofSize: 13, weight: .semibold)
    titleLabel.textColor = .labelColor
    titleLabel.lineBreakMode = .byTruncatingTail
    titleLabel.maximumNumberOfLines = 1
    titleLabel.cell?.usesSingleLineMode = true
    titleLabel.setContentCompressionResistancePriority(.required, for: .vertical)
    titleLabel.identifier = gatewayTitleIdentifier

    let detail = gatewayStatusDetail(
        title: title,
        endpoint: endpoint,
        statusDetail: statusDetail,
        isRunning: isRunning,
        isBusy: isBusy
    )
    let detailLabel = NSTextField(labelWithString: detail)
    detailLabel.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.lineBreakMode = .byTruncatingMiddle
    detailLabel.maximumNumberOfLines = 1
    detailLabel.cell?.usesSingleLineMode = true
    detailLabel.setContentCompressionResistancePriority(.required, for: .vertical)
    detailLabel.toolTip = statusDetail
    detailLabel.identifier = gatewayDetailIdentifier

    let textStack = NSStackView(views: [titleLabel, detailLabel])
    textStack.orientation = .vertical
    textStack.alignment = .leading
    textStack.spacing = 5
    textStack.translatesAutoresizingMaskIntoConstraints = false

    view.addSubview(statusDot)
    view.addSubview(textStack)
    view.addSubview(toggle)
    NSLayoutConstraint.activate([
        statusDot.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 14),
        statusDot.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        statusDot.widthAnchor.constraint(equalToConstant: 12),
        statusDot.heightAnchor.constraint(equalToConstant: 12),
        textStack.leadingAnchor.constraint(equalTo: statusDot.trailingAnchor, constant: 9),
        textStack.trailingAnchor.constraint(lessThanOrEqualTo: toggle.leadingAnchor, constant: -12),
        textStack.topAnchor.constraint(equalTo: view.topAnchor, constant: 12),
        textStack.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -12),
        titleLabel.widthAnchor.constraint(equalTo: textStack.widthAnchor),
        detailLabel.widthAnchor.constraint(equalTo: textStack.widthAnchor),
        toggle.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -8),
        toggle.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        toggle.widthAnchor.constraint(equalToConstant: 52),
        toggle.heightAnchor.constraint(equalToConstant: 30),
    ])
    return view
}

class FlippedMenuView: NSView {
    override var isFlipped: Bool { true }
}

private struct ProviderUsageGroup {
    let providerID: String
    let displayName: String
    let isEnabled: Bool
    let websiteURL: String?
    let quotas: [ProviderQuotaUsage]
    let models: [ProviderTokenUsage]
}

struct ProviderDashboardProvider: Equatable {
    let id: String
    let displayName: String
    let isEnabled: Bool
    let websiteURL: String?

    init(id: String, displayName: String, isEnabled: Bool = true, websiteURL: String? = nil) {
        self.id = id
        self.displayName = displayName
        self.isEnabled = isEnabled
        self.websiteURL = websiteURL
    }
}

private func providerLogoAssetName(_ providerID: String) -> String? {
    let normalized = providerID.lowercased()
    if normalized.contains("baidu") { return "baidu" }
    if normalized.contains("deepseek") { return "deepseek" }
    if normalized.contains("opencode") { return "opencode" }
    if normalized.contains("openrouter") { return "openrouter" }
    if normalized.contains("openai") || normalized.contains("chatgpt") { return "openai" }
    return "custom"
}

private func providerBrandColor(_ providerID: String) -> NSColor {
    let normalized = providerID.lowercased()
    if normalized.contains("baidu") {
        return NSColor(calibratedRed: 0.16, green: 0.29, blue: 0.93, alpha: 1)
    }
    if normalized.contains("deepseek") {
        return NSColor(calibratedRed: 0.34, green: 0.53, blue: 1, alpha: 1)
    }
    if normalized.contains("opencode") { return .labelColor }
    if normalized.contains("openrouter") {
        return NSColor(calibratedRed: 0.42, green: 0.44, blue: 0.95, alpha: 1)
    }
    if normalized.contains("openai") || normalized.contains("chatgpt") {
        return NSColor(calibratedRed: 0.10, green: 0.68, blue: 0.56, alpha: 1)
    }
    return .secondaryLabelColor
}

private func providerLogoImage(_ providerID: String, websiteURL: String?) -> NSImage? {
    if let cached = cachedProviderLogoImage(providerID: providerID, websiteURL: websiteURL) {
        return cached
    }
    guard let assetName = providerLogoAssetName(providerID) else { return nil }
    let directories = [
        Bundle.main.resourceURL?.appendingPathComponent("ProviderLogos", isDirectory: true),
        Bundle.main.bundleURL.appendingPathComponent("ProviderLogos", isDirectory: true),
    ]
    for directory in directories.compactMap({ $0 }) {
        let url = directory.appendingPathComponent("\(assetName).svg")
        if let image = NSImage(contentsOf: url) {
            image.isTemplate = true
            return image
        }
    }
    return nil
}

private func providerFallbackImage(_ providerID: String) -> NSImage {
    let color = providerBrandColor(providerID)
    let image = NSImage(size: NSSize(width: 24, height: 24))
    image.lockFocus()
    let body = NSBezierPath(roundedRect: NSRect(x: 1, y: 1, width: 22, height: 22), xRadius: 7, yRadius: 7)
    color.withAlphaComponent(0.16).setFill()
    body.fill()
    let ring = NSBezierPath(ovalIn: NSRect(x: 6, y: 6, width: 12, height: 12))
    color.withAlphaComponent(0.8).setStroke()
    ring.lineWidth = 1.5
    ring.stroke()
    let center = NSBezierPath(ovalIn: NSRect(x: 10, y: 10, width: 4, height: 4))
    color.setFill()
    center.fill()
    image.unlockFocus()
    return image
}

private func providerMonogram(_ providerID: String) -> String {
    let letters = providerID
        .split(separator: "-")
        .prefix(2)
        .compactMap(\.first)
        .map(String.init)
        .joined()
    return (letters.isEmpty ? String(providerID.prefix(2)) : letters).uppercased()
}

private func providerQuotaText(_ usage: ProviderQuotaUsage) -> String {
    let currency = usage.currency.map { " \($0)" } ?? ""
    if let used = usage.used, let limit = usage.limit {
        return "\(formatQuotaAmount(used)) / \(formatQuotaAmount(limit))\(currency)"
    }
    if let used = usage.used {
        return "\(formatQuotaAmount(used))\(currency)"
    }
    if let remaining = usage.remaining {
        return AppLocalization.string("menuViews.balance", formatQuotaAmount(remaining), currency)
    }
    if usage.error?.contains("not configured") == true {
        return AppLocalization.string("menuViews.quotaEndpointNotConfigured")
    }
    return AppLocalization.string("menuViews.queryFailed")
}

private func providerQuotaLabel(_ usage: ProviderQuotaUsage, multiple: Bool) -> String {
    switch usage.quotaID {
    case "five_hour": return "5h"
    case "weekly": return "1 周"
    case "monthly": return "月度"
    case "balance": return "余额"
    case "quota": return multiple ? (usage.label ?? "额度") : "额度"
    default:
        if let label = usage.label?.trimmingCharacters(in: .whitespacesAndNewlines),
           !label.isEmpty {
            return label
        }
        return usage.menuLabel
    }
}

private final class ProviderQuotaRowView: FlippedMenuView {
    init(frame: NSRect, usage: ProviderQuotaUsage, multiple: Bool) {
        super.init(frame: frame)
        identifier = NSUserInterfaceItemIdentifier(
            "provider-quota-\(usage.providerID)-\(usage.quotaID ?? "quota")"
        )
        toolTip = usage.error

        let label = NSTextField(labelWithString: providerQuotaLabel(usage, multiple: multiple))
        label.frame = NSRect(x: 2, y: 2, width: 118, height: 15)
        label.font = .systemFont(ofSize: 10, weight: .semibold)
        label.lineBreakMode = .byTruncatingTail
        addSubview(label)

        let value = NSTextField(labelWithString: providerQuotaText(usage))
        value.frame = NSRect(x: 122, y: 2, width: frame.width - 124, height: 15)
        value.font = .monospacedDigitSystemFont(ofSize: 10, weight: .medium)
        value.textColor = usage.used == nil && usage.remaining == nil
            ? .secondaryLabelColor
            : .labelColor
        value.alignment = .right
        value.lineBreakMode = .byTruncatingTail
        value.toolTip = usage.error
        addSubview(value)

        if let used = usage.used, let limit = usage.limit, limit > 0 {
            let progress = NSProgressIndicator(frame: NSRect(
                x: 2,
                y: 19,
                width: frame.width - 4,
                height: 6
            ))
            progress.identifier = NSUserInterfaceItemIdentifier("provider-quota-progress")
            progress.isIndeterminate = false
            progress.style = .bar
            progress.minValue = 0
            progress.maxValue = 1
            progress.doubleValue = min(max(used / limit, 0), 1)
            addSubview(progress)

            let detailText: String
            if let remaining = usage.remaining {
                let currency = usage.currency.map { " \($0)" } ?? ""
                detailText = AppLocalization.string(
                    "menuViews.remaining",
                    formatQuotaAmount(remaining),
                    currency
                )
            } else if let resetAt = usage.resetAt {
                detailText = resetAt
            } else {
                detailText = ""
            }
            if !detailText.isEmpty {
                let detail = NSTextField(labelWithString: detailText)
                detail.frame = NSRect(x: 2, y: 27, width: frame.width - 4, height: 12)
                detail.font = .systemFont(ofSize: 8)
                detail.textColor = .tertiaryLabelColor
                detail.lineBreakMode = .byTruncatingTail
                addSubview(detail)
            }
        }
    }

    required init?(coder: NSCoder) {
        return nil
    }
}

private func tokenUsageDetail(_ usage: ProviderTokenUsage) -> String {
    let cacheRatio = usage.cacheHitPercent.map {
        String(format: "%.1f%%", $0)
    } ?? "未上报"
    return """
    输入 \(formatTokenCount(usage.inputTokens))
    缓存输入 \(formatTokenCount(usage.cacheReadTokens))
    输出 \(formatTokenCount(usage.outputTokens))
    缓存输出 \(formatTokenCount(usage.cacheCreationTokens))
    整体缓存比例 \(cacheRatio)
    """
}

private final class TokenFlowBarView: NSView {
    let usage: ProviderTokenUsage
    let maximumTokens: UInt64

    init(frame: NSRect, usage: ProviderTokenUsage, maximumTokens: UInt64) {
        self.usage = usage
        self.maximumTokens = maximumTokens
        super.init(frame: frame)
        toolTip = tokenUsageDetail(usage)
    }

    required init?(coder: NSCoder) {
        return nil
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        let trackRect = bounds.insetBy(dx: 0, dy: 10)
        let track = NSBezierPath(roundedRect: trackRect, xRadius: 5, yRadius: 5)
        NSColor.separatorColor.withAlphaComponent(0.28).setFill()
        track.fill()
        guard usage.totalTokens > 0, maximumTokens > 0 else { return }

        let fillWidth = trackRect.width
            * CGFloat(Double(usage.totalTokens) / Double(maximumTokens))
        let values = [
            usage.inputTokens,
            usage.cacheReadTokens,
            usage.outputTokens,
            usage.cacheCreationTokens,
        ]
        let colors: [NSColor] = [
            .systemBlue.withAlphaComponent(0.45),
            .systemBlue,
            .secondaryLabelColor.withAlphaComponent(0.38),
            .secondaryLabelColor.withAlphaComponent(0.18),
        ]
        NSGraphicsContext.saveGraphicsState()
        track.addClip()
        var x = trackRect.minX
        for (value, color) in zip(values, colors) where value > 0 {
            let width = fillWidth * CGFloat(Double(value) / Double(usage.totalTokens))
            color.withAlphaComponent(0.9).setFill()
            NSRect(x: x, y: trackRect.minY, width: width, height: trackRect.height).fill()
            x += width
        }
        NSGraphicsContext.restoreGraphicsState()
    }
}

private final class TokenModelRowView: NSControl {
    let usage: ProviderTokenUsage

    init(
        frame: NSRect,
        usage: ProviderTokenUsage,
        maximumTokens: UInt64,
        selected: Bool
    ) {
        self.usage = usage
        super.init(frame: frame)
        identifier = NSUserInterfaceItemIdentifier("token-model-\(usage.modelID)")
        toolTip = tokenUsageDetail(usage)
        wantsLayer = true
        layer?.cornerRadius = 7
        layer?.backgroundColor = selected
            ? NSColor.controlAccentColor.withAlphaComponent(0.12).cgColor
            : NSColor.clear.cgColor

        let modelLabel = NSTextField(labelWithString: usage.modelID)
        modelLabel.frame = NSRect(x: 4, y: 7, width: 108, height: 18)
        modelLabel.font = .systemFont(ofSize: 11, weight: selected ? .semibold : .medium)
        modelLabel.lineBreakMode = .byTruncatingMiddle
        modelLabel.toolTip = tokenUsageDetail(usage)
        addSubview(modelLabel)

        let flow = TokenFlowBarView(
            frame: NSRect(x: 114, y: 0, width: 146, height: tokenModelRowHeight),
            usage: usage,
            maximumTokens: maximumTokens
        )
        addSubview(flow)

        let totalLabel = NSTextField(labelWithString: formatTokenCount(usage.totalTokens))
        totalLabel.frame = NSRect(x: 262, y: 7, width: 50, height: 18)
        totalLabel.font = .monospacedDigitSystemFont(ofSize: 10, weight: .medium)
        totalLabel.textColor = .secondaryLabelColor
        totalLabel.alignment = .right
        addSubview(totalLabel)
    }

    required init?(coder: NSCoder) {
        return nil
    }

    override func mouseDown(with event: NSEvent) {
        sendAction(action, to: target)
    }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .pointingHand)
    }
}

final class ProviderUsageDashboardView: FlippedMenuView {
    private var configuredProviders: [ProviderDashboardProvider] = []
    private var quotaUsages: [ProviderQuotaUsage] = []
    private var tokenUsages: [ProviderTokenUsage] = []
    private var quotaStatusTitle = "额度：检查中..."
    private var quotaStatusDetail: String?
    private var tokenStatusTitle = "Token 使用：检查中..."
    private var tokenStatusDetail: String?
    private var selectedProviderID: String?
    private var selectedModelID: String?
    private var visibleModels: [ProviderTokenUsage] = []

    init() {
        super.init(frame: NSRect(
            x: 0,
            y: 0,
            width: menuContentWidth,
            height: providerDashboardMinimumHeight
        ))
        render()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        frame = NSRect(
            x: 0,
            y: 0,
            width: menuContentWidth,
            height: providerDashboardMinimumHeight
        )
        render()
    }

    func updateQuotaStatus(title: String, detail: String?) {
        quotaStatusTitle = title
        quotaStatusDetail = detail
        render()
    }

    func updateConfiguredProviders(_ providers: [ProviderDashboardProvider]) {
        configuredProviders = providers
        render()
    }

    func refreshProviderIcons() {
        render()
    }

    func updateQuotaUsages(_ usages: [ProviderQuotaUsage]) {
        quotaUsages = usages
        quotaStatusTitle = L10n.Provider.quotaEmpty
        quotaStatusDetail = nil
        render()
    }

    func updateTokenStatus(title: String, detail: String?) {
        tokenStatusTitle = title
        tokenStatusDetail = detail
        render()
    }

    func updateTokenUsages(_ usages: [ProviderTokenUsage]) {
        tokenUsages = usages
        tokenStatusTitle = "Token 使用：暂无数据"
        tokenStatusDetail = nil
        render()
    }

    private func usageGroups() -> [ProviderUsageGroup] {
        configuredProviders.filter(\.isEnabled).map { configuredProvider in
            let providerID = configuredProvider.id
            let quotas = quotaUsages.filter { $0.providerID == providerID }
            let models = tokenUsages
                .filter { $0.providerID == providerID }
                .sorted {
                    $0.totalTokens == $1.totalTokens
                        ? $0.modelID < $1.modelID
                        : $0.totalTokens > $1.totalTokens
                }
            return ProviderUsageGroup(
                providerID: providerID,
                displayName: configuredProvider.displayName,
                isEnabled: true,
                websiteURL: configuredProvider.websiteURL,
                quotas: quotas,
                models: models
            )
        }
    }

    private func render() {
        subviews.forEach { $0.removeFromSuperview() }
        frame.size = NSSize(width: menuContentWidth, height: providerDashboardMinimumHeight)
        let groups = usageGroups()
        if selectedProviderID == nil
            || !groups.contains(where: { $0.providerID == selectedProviderID })
        {
            selectedProviderID = groups.first?.providerID
            selectedModelID = nil
        }

        let title = NSTextField(labelWithString: "Provider 使用")
        title.frame = NSRect(x: 12, y: 9, width: 180, height: 18)
        title.font = .systemFont(ofSize: 12, weight: .semibold)
        addSubview(title)

        renderProviderTabs(groups)
        guard let group = groups.first(where: { $0.providerID == selectedProviderID }) else {
            renderEmptyState()
            return
        }
        let quotaBottom = renderProviderSummary(group)
        guard !group.models.isEmpty else {
            let message = NSTextField(labelWithString: tokenStatusTitle)
            message.frame = NSRect(x: 12, y: quotaBottom + 10, width: 312, height: 18)
            message.font = .systemFont(ofSize: 10)
            message.textColor = .tertiaryLabelColor
            message.alignment = .center
            message.toolTip = tokenStatusDetail
            addSubview(message)
            frame.size.height = max(providerDashboardMinimumHeight, quotaBottom + 38)
            return
        }
        let chartBottom = renderModelChart(group.models, at: quotaBottom + 8)
        if selectedModelID != nil {
            renderModelDetail(group.models, at: chartBottom + 6)
            frame.size.height = chartBottom + 74
        } else {
            frame.size.height = chartBottom + 8
        }
    }

    private func renderProviderTabs(_ groups: [ProviderUsageGroup]) {
        let scroll = NSScrollView(frame: NSRect(x: 10, y: 28, width: 316, height: 36))
        scroll.identifier = NSUserInterfaceItemIdentifier("provider-tab-scroll")
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasHorizontalScroller = groups.count * Int(providerTabWidth) > Int(scroll.frame.width)
        scroll.hasVerticalScroller = false
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay

        let documentWidth = max(scroll.frame.width, CGFloat(groups.count) * providerTabWidth)
        let document = FlippedMenuView(frame: NSRect(
            x: 0,
            y: 0,
            width: documentWidth,
            height: scroll.frame.height
        ))
        for (index, group) in groups.enumerated() {
            let selected = group.providerID == selectedProviderID
            let brandColor = providerBrandColor(group.providerID)
            let logo = providerLogoImage(group.providerID, websiteURL: group.websiteURL)
                ?? providerFallbackImage(group.providerID)
            let button = NSButton(
                title: "",
                target: self,
                action: #selector(selectProvider(_:))
            )
            button.frame = NSRect(
                x: CGFloat(index) * providerTabWidth + 3,
                y: 3,
                width: 30,
                height: 30
            )
            button.tag = index
            button.identifier = NSUserInterfaceItemIdentifier("provider-tab-\(group.providerID)")
            button.image = logo
            button.imagePosition = .imageOnly
            button.imageScaling = .scaleProportionallyDown
            button.font = .systemFont(ofSize: 9, weight: .bold)
            button.isBordered = false
            button.contentTintColor = brandColor
            button.toolTip = group.displayName
            button.alphaValue = group.isEnabled ? 1 : 0.42
            button.setAccessibilityLabel(group.displayName)
            button.wantsLayer = true
            button.layer?.cornerRadius = 9
            button.layer?.backgroundColor = selected
                ? brandColor.withAlphaComponent(0.13).cgColor
                : NSColor.clear.cgColor
            button.layer?.borderWidth = selected ? 1 : 0
            button.layer?.borderColor = brandColor.withAlphaComponent(0.32).cgColor
            document.addSubview(button)
        }
        scroll.documentView = document
        addSubview(scroll)
    }

    private func renderProviderSummary(_ group: ProviderUsageGroup) -> CGFloat {
        let divider = NSBox(frame: NSRect(x: 12, y: 70, width: 312, height: 1))
        divider.boxType = .separator
        addSubview(divider)

        let icon = NSImageView(frame: NSRect(x: 12, y: 80, width: 22, height: 22))
        icon.image = providerLogoImage(group.providerID, websiteURL: group.websiteURL)
            ?? providerFallbackImage(group.providerID)
        icon.imageScaling = .scaleProportionallyDown
        icon.contentTintColor = providerBrandColor(group.providerID)
        icon.alphaValue = group.isEnabled ? 1 : 0.42
        addSubview(icon)

        let displayName = group.isEnabled
            ? group.displayName
            : "\(group.displayName)（已停用）"
        let name = NSTextField(labelWithString: displayName)
        name.frame = NSRect(x: 42, y: 81, width: 282, height: 20)
        name.font = .systemFont(ofSize: 13, weight: .semibold)
        name.lineBreakMode = .byTruncatingMiddle
        name.textColor = group.isEnabled ? .labelColor : .tertiaryLabelColor
        addSubview(name)

        let quotaTop: CGFloat = 106
        guard !group.quotas.isEmpty else {
            let status = NSTextField(
                labelWithString: group.isEnabled ? quotaStatusTitle : "额度：已停用"
            )
            status.frame = NSRect(x: 12, y: quotaTop, width: 312, height: 18)
            status.font = .systemFont(ofSize: 10)
            status.textColor = .secondaryLabelColor
            status.toolTip = quotaStatusDetail
            addSubview(status)
            return quotaTop + 24
        }

        let visibleRows = min(group.quotas.count, visibleProviderQuotaRows)
        let quotaHeight = CGFloat(visibleRows) * providerQuotaRowHeight
        let scroll = NSScrollView(frame: NSRect(
            x: 10,
            y: quotaTop,
            width: 316,
            height: quotaHeight
        ))
        scroll.identifier = NSUserInterfaceItemIdentifier("provider-quota-scroll")
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasHorizontalScroller = false
        scroll.hasVerticalScroller = group.quotas.count > visibleProviderQuotaRows
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay

        let document = FlippedMenuView(frame: NSRect(
            x: 0,
            y: 0,
            width: scroll.frame.width,
            height: CGFloat(group.quotas.count) * providerQuotaRowHeight
        ))
        let multiple = group.quotas.count > 1
        for (index, usage) in group.quotas.enumerated() {
            let row = ProviderQuotaRowView(
                frame: NSRect(
                    x: 0,
                    y: CGFloat(index) * providerQuotaRowHeight,
                    width: scroll.frame.width,
                    height: providerQuotaRowHeight
                ),
                usage: usage,
                multiple: multiple
            )
            document.addSubview(row)
        }
        scroll.documentView = document
        addSubview(scroll)
        return quotaTop + quotaHeight
    }

    private func renderModelChart(_ models: [ProviderTokenUsage], at y: CGFloat) -> CGFloat {
        visibleModels = models
        if let selectedModelID,
           !models.contains(where: { $0.modelID == selectedModelID })
        {
            self.selectedModelID = nil
        }

        let visibleRows = min(models.count, visibleTokenModelRows)
        let chartHeight = CGFloat(visibleRows) * tokenModelRowHeight
        let scroll = NSScrollView(frame: NSRect(x: 10, y: y, width: 316, height: chartHeight))
        scroll.identifier = NSUserInterfaceItemIdentifier("token-model-scroll")
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasHorizontalScroller = false
        scroll.hasVerticalScroller = models.count > visibleTokenModelRows
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay

        let documentHeight = max(
            scroll.frame.height,
            CGFloat(models.count) * tokenModelRowHeight
        )
        let document = FlippedMenuView(frame: NSRect(
            x: 0,
            y: 0,
            width: scroll.frame.width,
            height: documentHeight
        ))
        let maximumTokens = models.map(\.totalTokens).max() ?? 0
        for (index, usage) in models.enumerated() {
            let row = TokenModelRowView(
                frame: NSRect(
                    x: 0,
                    y: CGFloat(index) * tokenModelRowHeight,
                    width: scroll.frame.width,
                    height: tokenModelRowHeight
                ),
                usage: usage,
                maximumTokens: maximumTokens,
                selected: usage.modelID == selectedModelID
            )
            row.tag = index
            row.target = self
            row.action = #selector(selectModel(_:))
            document.addSubview(row)
        }
        scroll.documentView = document
        addSubview(scroll)
        return y + chartHeight
    }

    private func renderModelDetail(_ models: [ProviderTokenUsage], at y: CGFloat) {
        guard let usage = models.first(where: { $0.modelID == selectedModelID }) else { return }
        let panel = NSView(frame: NSRect(x: 10, y: y, width: 316, height: 58))
        panel.wantsLayer = true
        panel.layer?.cornerRadius = 9
        panel.layer?.backgroundColor = NSColor.controlBackgroundColor
            .withAlphaComponent(0.62)
            .cgColor
        panel.layer?.borderWidth = 0.75
        panel.layer?.borderColor = NSColor.separatorColor.withAlphaComponent(0.4).cgColor
        panel.toolTip = tokenUsageDetail(usage)

        let title = NSTextField(labelWithString:
            "\(usage.modelID) · \(usage.requestCount) 次请求")
        title.frame = NSRect(x: 10, y: 5, width: 296, height: 15)
        title.font = .systemFont(ofSize: 10, weight: .semibold)
        title.lineBreakMode = .byTruncatingMiddle
        panel.addSubview(title)

        let cacheRatio = usage.cacheHitPercent.map {
            String(format: "%.1f%%", $0)
        } ?? "未上报"
        let metrics = [
            ("输入", formatTokenCount(usage.inputTokens)),
            ("缓存输入", formatTokenCount(usage.cacheReadTokens)),
            ("输出", formatTokenCount(usage.outputTokens)),
            ("缓存输出", formatTokenCount(usage.cacheCreationTokens)),
            ("缓存比例", cacheRatio),
        ]
        let cellWidth: CGFloat = 59
        for (index, metric) in metrics.enumerated() {
            let x = 10 + CGFloat(index) * cellWidth
            let label = NSTextField(labelWithString: metric.0)
            label.frame = NSRect(x: x, y: 23, width: cellWidth - 4, height: 13)
            label.font = .systemFont(ofSize: 8)
            label.textColor = .secondaryLabelColor
            panel.addSubview(label)

            let value = NSTextField(labelWithString: metric.1)
            value.frame = NSRect(x: x, y: 36, width: cellWidth - 4, height: 16)
            value.font = .monospacedDigitSystemFont(ofSize: 10, weight: .semibold)
            panel.addSubview(value)
        }
        addSubview(panel)
    }

    private func renderEmptyState() {
        let message = NSTextField(labelWithString: tokenStatusTitle)
        message.frame = NSRect(x: 12, y: 94, width: 312, height: 20)
        message.font = .systemFont(ofSize: 12)
        message.textColor = .secondaryLabelColor
        message.alignment = .center
        message.toolTip = tokenStatusDetail ?? quotaStatusDetail
        addSubview(message)
    }

    @objc private func selectProvider(_ sender: NSButton) {
        let groups = usageGroups()
        guard groups.indices.contains(sender.tag) else { return }
        selectedProviderID = groups[sender.tag].providerID
        selectedModelID = nil
        render()
    }

    @objc private func selectModel(_ sender: TokenModelRowView) {
        guard visibleModels.indices.contains(sender.tag) else { return }
        let modelID = visibleModels[sender.tag].modelID
        selectedModelID = selectedModelID == modelID ? nil : modelID
        render()
    }
}

func formatTokenCount(_ count: UInt64) -> String {
    if count >= 1_000_000 {
        return String(format: "%.1fM", Double(count) / 1_000_000)
    }
    if count >= 1_000 {
        return String(format: "%.1fk", Double(count) / 1_000)
    }
    return "\(count)"
}


func formatQuotaAmount(_ value: Double) -> String {
    let formatter = NumberFormatter()
    formatter.minimumFractionDigits = value.rounded() == value ? 0 : 2
    formatter.maximumFractionDigits = 2
    return formatter.string(from: NSNumber(value: value)) ?? String(format: "%.2f", value)
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
