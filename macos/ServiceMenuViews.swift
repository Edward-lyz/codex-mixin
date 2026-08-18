import Cocoa

private let menuContentWidth: CGFloat = 336
private let serviceMenuHeight: CGFloat = 56
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
