import Cocoa

private let menuContentWidth: CGFloat = 392
private let serviceMenuHeight: CGFloat = 56
private let providerDashboardHeight: CGFloat = 354
private let providerTabWidth: CGFloat = 44
private let tokenModelRowHeight: CGFloat = 32
private let visibleTokenModelRows = 4
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
    let quota: ProviderQuotaUsage?
    let models: [ProviderTokenUsage]
}

private func providerSymbolName(_ providerID: String) -> String {
    let normalized = providerID.lowercased()
    if normalized.contains("baidu") { return "cloud.fill" }
    if normalized.contains("openai") || normalized.contains("chatgpt") { return "sparkles" }
    if normalized.contains("deepseek") { return "scope" }
    if normalized.contains("opencode") { return "terminal.fill" }
    if normalized.contains("anthropic") || normalized.contains("claude") { return "brain.head.profile" }
    return "cube.fill"
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
            .systemBlue,
            .systemGreen,
            .systemOrange,
            .systemYellow,
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
        modelLabel.frame = NSRect(x: 4, y: 7, width: 126, height: 18)
        modelLabel.font = .systemFont(ofSize: 11, weight: selected ? .semibold : .medium)
        modelLabel.lineBreakMode = .byTruncatingMiddle
        modelLabel.toolTip = tokenUsageDetail(usage)
        addSubview(modelLabel)

        let flow = TokenFlowBarView(
            frame: NSRect(x: 132, y: 0, width: 184, height: tokenModelRowHeight),
            usage: usage,
            maximumTokens: maximumTokens
        )
        addSubview(flow)

        let totalLabel = NSTextField(labelWithString: formatTokenCount(usage.totalTokens))
        totalLabel.frame = NSRect(x: 318, y: 7, width: 54, height: 18)
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
            height: providerDashboardHeight
        ))
        render()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        frame = NSRect(
            x: 0,
            y: 0,
            width: menuContentWidth,
            height: providerDashboardHeight
        )
        render()
    }

    func updateQuotaStatus(title: String, detail: String?) {
        quotaUsages = []
        quotaStatusTitle = title
        quotaStatusDetail = detail
        render()
    }

    func updateQuotaUsages(_ usages: [ProviderQuotaUsage]) {
        quotaUsages = usages
        quotaStatusTitle = L10n.Provider.quotaEmpty
        quotaStatusDetail = nil
        render()
    }

    func updateTokenStatus(title: String, detail: String?) {
        tokenUsages = []
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
        var providerIDs = quotaUsages.map(\.providerID)
        for providerID in tokenUsages.map(\.providerID).sorted()
            where !providerIDs.contains(providerID)
        {
            providerIDs.append(providerID)
        }
        return providerIDs.map { providerID in
            let quota = quotaUsages.first { $0.providerID == providerID }
            let models = tokenUsages
                .filter { $0.providerID == providerID }
                .sorted {
                    $0.totalTokens == $1.totalTokens
                        ? $0.modelID < $1.modelID
                        : $0.totalTokens > $1.totalTokens
                }
            return ProviderUsageGroup(
                providerID: providerID,
                displayName: quota?.menuLabel ?? providerID,
                quota: quota,
                models: models
            )
        }
    }

    private func render() {
        subviews.forEach { $0.removeFromSuperview() }
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
        renderProviderSummary(group)
        renderLegend()
        renderModelChart(group.models)
        renderModelDetail(group.models)
    }

    private func renderProviderTabs(_ groups: [ProviderUsageGroup]) {
        let scroll = NSScrollView(frame: NSRect(x: 10, y: 32, width: 372, height: 42))
        scroll.identifier = NSUserInterfaceItemIdentifier("provider-tab-scroll")
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasHorizontalScroller = false
        scroll.hasVerticalScroller = false
        scroll.horizontalScrollElasticity = .automatic

        let documentWidth = max(scroll.frame.width, CGFloat(groups.count) * providerTabWidth)
        let document = FlippedMenuView(frame: NSRect(
            x: 0,
            y: 0,
            width: documentWidth,
            height: scroll.frame.height
        ))
        for (index, group) in groups.enumerated() {
            let selected = group.providerID == selectedProviderID
            let button = NSButton(
                title: "",
                target: self,
                action: #selector(selectProvider(_:))
            )
            button.frame = NSRect(
                x: CGFloat(index) * providerTabWidth + 4,
                y: 3,
                width: 36,
                height: 36
            )
            button.tag = index
            button.identifier = NSUserInterfaceItemIdentifier("provider-tab-\(group.providerID)")
            button.image = menuItemImage(providerSymbolName(group.providerID))
            button.imagePosition = .imageOnly
            button.isBordered = false
            button.contentTintColor = selected ? .controlAccentColor : .secondaryLabelColor
            button.toolTip = group.displayName
            button.setAccessibilityLabel(group.displayName)
            button.wantsLayer = true
            button.layer?.cornerRadius = 10
            button.layer?.backgroundColor = selected
                ? NSColor.controlAccentColor.withAlphaComponent(0.14).cgColor
                : NSColor.clear.cgColor
            button.layer?.borderWidth = selected ? 1 : 0
            button.layer?.borderColor = NSColor.controlAccentColor.withAlphaComponent(0.28).cgColor
            document.addSubview(button)
        }
        scroll.documentView = document
        addSubview(scroll)
    }

    private func renderProviderSummary(_ group: ProviderUsageGroup) {
        let divider = NSBox(frame: NSRect(x: 12, y: 78, width: 368, height: 1))
        divider.boxType = .separator
        addSubview(divider)

        let icon = NSImageView(frame: NSRect(x: 12, y: 88, width: 26, height: 26))
        icon.image = menuItemImage(providerSymbolName(group.providerID))
        icon.contentTintColor = .controlAccentColor
        addSubview(icon)

        let name = NSTextField(labelWithString: group.displayName)
        name.frame = NSRect(x: 46, y: 86, width: 196, height: 19)
        name.font = .systemFont(ofSize: 13, weight: .semibold)
        name.lineBreakMode = .byTruncatingMiddle
        addSubview(name)

        let providerID = NSTextField(labelWithString: group.providerID)
        providerID.frame = NSRect(x: 46, y: 104, width: 196, height: 15)
        providerID.font = .monospacedSystemFont(ofSize: 9, weight: .regular)
        providerID.textColor = .tertiaryLabelColor
        providerID.lineBreakMode = .byTruncatingMiddle
        addSubview(providerID)

        let quotaText = group.quota.map(providerQuotaText) ?? quotaStatusTitle
        let quota = NSTextField(labelWithString: quotaText)
        quota.frame = NSRect(x: 246, y: 90, width: 134, height: 18)
        quota.font = .monospacedDigitSystemFont(ofSize: 10, weight: .medium)
        quota.textColor = group.quota == nil ? .secondaryLabelColor : .labelColor
        quota.alignment = .right
        quota.lineBreakMode = .byTruncatingTail
        quota.toolTip = group.quota?.error ?? quotaStatusDetail
        addSubview(quota)
    }

    private func renderLegend() {
        let entries: [(String, NSColor)] = [
            ("输入", .systemBlue),
            ("缓存输入", .systemGreen),
            ("输出", .systemOrange),
            ("缓存输出", .systemYellow),
        ]
        var x: CGFloat = 12
        for (label, color) in entries {
            let dot = NSView(frame: NSRect(x: x, y: 132, width: 7, height: 7))
            dot.wantsLayer = true
            dot.layer?.cornerRadius = 3.5
            dot.layer?.backgroundColor = color.cgColor
            dot.toolTip = label == "缓存输出"
                ? "缓存输出对应上游 cache_creation_tokens"
                : nil
            addSubview(dot)

            let text = NSTextField(labelWithString: label)
            let width = label == "输入" || label == "输出" ? 28.0 : 52.0
            text.frame = NSRect(x: x + 11, y: 127, width: width, height: 16)
            text.font = .systemFont(ofSize: 9)
            text.textColor = .secondaryLabelColor
            text.toolTip = dot.toolTip
            addSubview(text)
            x += width + 20
        }
    }

    private func renderModelChart(_ models: [ProviderTokenUsage]) {
        visibleModels = models
        if selectedModelID == nil
            || !models.contains(where: { $0.modelID == selectedModelID })
        {
            selectedModelID = models.first?.modelID
        }

        let scroll = NSScrollView(frame: NSRect(x: 10, y: 148, width: 372, height: 132))
        scroll.identifier = NSUserInterfaceItemIdentifier("token-model-scroll")
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasHorizontalScroller = false
        scroll.hasVerticalScroller = models.count > visibleTokenModelRows
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay

        guard !models.isEmpty else {
            let message = NSTextField(labelWithString: tokenStatusTitle)
            message.frame = NSRect(x: 8, y: 45, width: 350, height: 18)
            message.font = .systemFont(ofSize: 11)
            message.textColor = .secondaryLabelColor
            message.alignment = .center
            message.toolTip = tokenStatusDetail
            let document = FlippedMenuView(frame: scroll.bounds)
            document.addSubview(message)
            scroll.documentView = document
            addSubview(scroll)
            return
        }

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
    }

    private func renderModelDetail(_ models: [ProviderTokenUsage]) {
        guard let usage = models.first(where: { $0.modelID == selectedModelID }) else { return }
        let panel = NSView(frame: NSRect(x: 10, y: 286, width: 372, height: 58))
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
        title.frame = NSRect(x: 10, y: 5, width: 352, height: 15)
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
        let cellWidth: CGFloat = 70
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
        message.frame = NSRect(x: 20, y: 154, width: 352, height: 20)
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
        selectedModelID = visibleModels[sender.tag].modelID
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
