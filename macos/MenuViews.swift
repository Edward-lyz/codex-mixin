import Cocoa

func serviceMenuView(
    title: String,
    endpoint: String?,
    isRunning: Bool,
    isBusy: Bool
) -> NSView {
    let view = NSView(frame: NSRect(x: 0, y: 0, width: 320, height: 56))
    let statusColor: NSColor
    if title.contains("失败") {
        statusColor = .systemRed
    } else if title.contains("等待配置") || title.contains("降级") || title.contains("无启用") {
        statusColor = .systemOrange
    } else if isBusy {
        statusColor = .systemOrange
    } else if isRunning {
        statusColor = .systemGreen
    } else {
        statusColor = .systemGray
    }

    let statusDot = NSView()
    statusDot.wantsLayer = true
    statusDot.layer?.cornerRadius = 9
    statusDot.layer?.backgroundColor = statusColor.cgColor
    statusDot.layer?.borderWidth = 3
    statusDot.layer?.borderColor = statusColor.withAlphaComponent(0.28).cgColor
    statusDot.layer?.shadowColor = statusColor.cgColor
    statusDot.layer?.shadowOpacity = isRunning ? 0.45 : 0
    statusDot.layer?.shadowRadius = 3
    statusDot.translatesAutoresizingMaskIntoConstraints = false

    let titleLabel = NSTextField(labelWithString: title)
    titleLabel.font = .boldSystemFont(ofSize: 13)
    titleLabel.textColor = .labelColor
    titleLabel.lineBreakMode = .byTruncatingTail

    let detail: String
    if let endpoint {
        detail = endpoint
    } else if title.contains("失败") {
        detail = "请查看运行日志"
    } else if title.contains("等待配置") {
        detail = "请先设置供应商与 API Key"
    } else if isBusy {
        detail = "正在分配本地端口"
    } else if isRunning {
        detail = "正在读取本地接口地址"
    } else {
        detail = "网关当前未运行"
    }
    let detailLabel = NSTextField(labelWithString: detail)
    detailLabel.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.lineBreakMode = .byTruncatingMiddle

    let textStack = NSStackView(views: [titleLabel, detailLabel])
    textStack.orientation = .vertical
    textStack.alignment = .leading
    textStack.spacing = 3
    textStack.translatesAutoresizingMaskIntoConstraints = false

    view.addSubview(statusDot)
    view.addSubview(textStack)
    NSLayoutConstraint.activate([
        statusDot.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 12),
        statusDot.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        statusDot.widthAnchor.constraint(equalToConstant: 18),
        statusDot.heightAnchor.constraint(equalToConstant: 18),
        textStack.leadingAnchor.constraint(equalTo: statusDot.trailingAnchor, constant: 11),
        textStack.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -12),
        textStack.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        titleLabel.widthAnchor.constraint(equalTo: textStack.widthAnchor),
        detailLabel.widthAnchor.constraint(equalTo: textStack.widthAnchor),
    ])
    return view
}

func quotaMenuView(title: String, detail: String?, progress: Double?) -> NSView {
    let view = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: detail == nil ? 34 : 48))
    let icon = NSImageView(image: menuItemImage("chart.bar") ?? NSImage())
    icon.translatesAutoresizingMaskIntoConstraints = false
    icon.widthAnchor.constraint(equalToConstant: 18).isActive = true
    icon.heightAnchor.constraint(equalToConstant: 18).isActive = true

    let titleLabel = NSTextField(labelWithString: title)
    titleLabel.font = .systemFont(ofSize: NSFont.systemFontSize)
    titleLabel.lineBreakMode = .byTruncatingTail
    titleLabel.translatesAutoresizingMaskIntoConstraints = false

    let progressBar = NSProgressIndicator()
    progressBar.isIndeterminate = progress == nil
    progressBar.style = .bar
    progressBar.minValue = 0
    progressBar.maxValue = 1
    progressBar.doubleValue = min(max(progress ?? 0, 0), 1)
    progressBar.translatesAutoresizingMaskIntoConstraints = false
    progressBar.heightAnchor.constraint(equalToConstant: 8).isActive = true

    let rows: [NSView]
    if let detail {
        let detailLabel = NSTextField(labelWithString: detail)
        detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        detailLabel.textColor = .secondaryLabelColor
        rows = [titleLabel, progressBar, detailLabel]
    } else {
        rows = [titleLabel, progressBar]
    }
    let textStack = NSStackView(views: rows)
    textStack.orientation = .vertical
    textStack.alignment = .leading
    textStack.spacing = 3
    textStack.translatesAutoresizingMaskIntoConstraints = false

    view.addSubview(icon)
    view.addSubview(textStack)
    NSLayoutConstraint.activate([
        icon.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 9),
        icon.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        textStack.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 8),
        textStack.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -10),
        textStack.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        titleLabel.widthAnchor.constraint(equalTo: textStack.widthAnchor),
        progressBar.widthAnchor.constraint(equalTo: textStack.widthAnchor),
    ])
    return view
}

func providerQuotaMenuView(_ usages: [ProviderQuotaUsage]) -> NSView {
    if usages.isEmpty {
        return quotaMenuView(title: "Provider 额度：无可查询项", detail: nil, progress: nil)
    }
    let rowHeight: CGFloat = 38
    let view = NSView(frame: NSRect(
        x: 0,
        y: 0,
        width: 320,
        height: max(rowHeight * CGFloat(usages.count), rowHeight)
    ))
    let rows = usages.map(providerQuotaRow)
    let stack = NSStackView(views: rows)
    stack.orientation = .vertical
    stack.alignment = .leading
    stack.distribution = .fillEqually
    stack.spacing = 0
    stack.translatesAutoresizingMaskIntoConstraints = false
    view.addSubview(stack)
    NSLayoutConstraint.activate([
        stack.leadingAnchor.constraint(equalTo: view.leadingAnchor),
        stack.trailingAnchor.constraint(equalTo: view.trailingAnchor),
        stack.topAnchor.constraint(equalTo: view.topAnchor),
        stack.bottomAnchor.constraint(equalTo: view.bottomAnchor),
    ])
    return view
}

func providerQuotaRow(_ usage: ProviderQuotaUsage) -> NSView {
    let icon = NSImageView(image: menuItemImage("chart.bar") ?? NSImage())
    icon.translatesAutoresizingMaskIntoConstraints = false
    icon.widthAnchor.constraint(equalToConstant: 16).isActive = true
    icon.heightAnchor.constraint(equalToConstant: 16).isActive = true

    let providerLabel = NSTextField(labelWithString: usage.providerID)
    providerLabel.font = .monospacedSystemFont(ofSize: 11, weight: .medium)
    providerLabel.lineBreakMode = .byTruncatingMiddle

    let value: String
    if let amount = usage.value {
        let currency = usage.currency.map { " \($0)" } ?? ""
        value = "\(formatQuotaAmount(amount))\(currency)"
    } else if usage.error?.contains("not configured") == true {
        value = "未配置额度接口"
    } else {
        value = "查询失败"
    }
    let valueLabel = NSTextField(labelWithString: value)
    valueLabel.font = .systemFont(ofSize: 11)
    valueLabel.textColor = usage.value == nil ? .secondaryLabelColor : .labelColor
    valueLabel.alignment = .right
    valueLabel.lineBreakMode = .byTruncatingTail
    valueLabel.toolTip = usage.error

    let row = NSStackView(views: [icon, providerLabel, NSView(), valueLabel])
    row.orientation = .horizontal
    row.alignment = .centerY
    row.spacing = 8
    row.translatesAutoresizingMaskIntoConstraints = false
    row.heightAnchor.constraint(equalToConstant: 38).isActive = true
    providerLabel.widthAnchor.constraint(greaterThanOrEqualToConstant: 100).isActive = true
    valueLabel.widthAnchor.constraint(greaterThanOrEqualToConstant: 90).isActive = true

    let container = NSView()
    container.addSubview(row)
    NSLayoutConstraint.activate([
        row.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 10),
        row.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -10),
        row.topAnchor.constraint(equalTo: container.topAnchor),
        row.bottomAnchor.constraint(equalTo: container.bottomAnchor),
    ])
    return container
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

    let body = NSBezierPath(roundedRect: NSRect(x: 2.2, y: 2.0, width: 17.8, height: 17.8), xRadius: 6.0, yRadius: 6.0)
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
