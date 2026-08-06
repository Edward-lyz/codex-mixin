import Cocoa

final class InstallProgressWindowController: NSWindowController {
    private let phaseLabel = NSTextField(labelWithString: "准备中...")
    private let detailLabel = NSTextField(labelWithString: "安装期间请勿打开 Codex App")
    private let progress = NSProgressIndicator()
    private let elapsedLabel = NSTextField(labelWithString: "")
    private let startedAt = Date()
    private var timer: Timer?
    private let stepsStack = NSStackView()
    private var phases: [String] = []
    private var currentPhaseIndex: Int?
    private var stepDots: [NSView] = []
    private var stepLabels: [NSTextField] = []

    init(title: String = "正在安装到 Codex") {
        let content = NSView(frame: NSRect(x: 0, y: 0, width: 500, height: 170))
        let window = NSPanel(contentRect: content.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.title = title
        window.isReleasedWhenClosed = false
        window.contentView = content
        super.init(window: window)

        phaseLabel.font = .boldSystemFont(ofSize: 16)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.font = .systemFont(ofSize: 12)
        progress.style = .bar
        progress.isIndeterminate = true
        progress.controlSize = .regular
        progress.startAnimation(nil)
        elapsedLabel.textColor = .tertiaryLabelColor
        elapsedLabel.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)

        stepsStack.orientation = .vertical
        stepsStack.alignment = .leading
        stepsStack.spacing = 6
        stepsStack.isHidden = true

        let stack = NSStackView(views: [phaseLabel, progress, detailLabel, elapsedLabel])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false

        let body = NSStackView(views: [stepsStack, stack])
        body.orientation = .horizontal
        body.alignment = .top
        body.spacing = 20
        body.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(body)
        NSLayoutConstraint.activate([
            body.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 28),
            body.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -28),
            body.centerYAnchor.constraint(equalTo: content.centerYAnchor),
            progress.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func present() {
        showWindow(nil)
        window?.center()
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        timer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.elapsedLabel.stringValue = String(format: "已用时 %.1fs", Date().timeIntervalSince(self.startedAt))
        }
    }

    func update(phase: String) {
        phaseLabel.stringValue = phase
        if !phases.isEmpty, let index = phases.firstIndex(of: phase) {
            currentPhaseIndex = index
            refreshStepIndicator()
        }
    }

    func finish() {
        timer?.invalidate()
        timer = nil
        progress.isIndeterminate = false
        progress.maxValue = 1
        progress.doubleValue = 1
        phaseLabel.textColor = .mixinHealthy
        phaseLabel.stringValue = "✓ 安装完成"
        if !phases.isEmpty {
            currentPhaseIndex = phases.count - 1
            refreshStepIndicator()
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
            self?.close()
        }
    }

    func fail(message: String) {
        timer?.invalidate()
        timer = nil
        progress.stopAnimation(nil)
        progress.isHidden = true
        phaseLabel.textColor = .mixinError
        phaseLabel.stringValue = "✗ 安装失败"
        detailLabel.textColor = .mixinError
        detailLabel.stringValue = message
        window?.styleMask.insert(.closable)
    }

    func setPhases(_ phases: [String]) {
        self.phases = phases
        stepsStack.arrangedSubviews.forEach { stepsStack.removeArrangedSubview($0); $0.removeFromSuperview() }
        stepDots.removeAll()
        stepLabels.removeAll()
        guard !phases.isEmpty else {
            stepsStack.isHidden = true
            return
        }
        for phase in phases {
            let dot = NSView()
            dot.wantsLayer = true
            dot.layer?.cornerRadius = 5
            dot.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                dot.widthAnchor.constraint(equalToConstant: 10),
                dot.heightAnchor.constraint(equalToConstant: 10),
            ])
            let label = NSTextField(labelWithString: phase)
            label.font = .systemFont(ofSize: 12)
            label.lineBreakMode = .byTruncatingTail
            let row = NSStackView(views: [dot, label])
            row.orientation = .horizontal
            row.alignment = .centerY
            row.spacing = 8
            stepsStack.addArrangedSubview(row)
            stepDots.append(dot)
            stepLabels.append(label)
        }
        stepsStack.isHidden = false
        currentPhaseIndex = nil
        refreshStepIndicator()
        window?.setContentSize(NSSize(width: 500, height: 220))
    }

    private func refreshStepIndicator() {
        for (index, (dot, label)) in zip(stepDots, stepLabels).enumerated() {
            let state: (dotColor: NSColor?, labelColor: NSColor, bold: Bool)
            if let current = currentPhaseIndex, index < current {
                state = (.mixinHealthy, .secondaryLabelColor, false)
            } else if let current = currentPhaseIndex, index == current {
                state = (.mixinAccent, .labelColor, true)
            } else {
                state = (nil, .tertiaryLabelColor, false)
            }
            dot.layer?.backgroundColor = state.dotColor?.cgColor
            dot.layer?.borderWidth = state.dotColor == nil ? 1.5 : 0
            dot.layer?.borderColor = NSColor.mixinIdle.cgColor
            label.textColor = state.labelColor
            label.font = state.bold ? .boldSystemFont(ofSize: 12) : .systemFont(ofSize: 12)
        }
    }
}
