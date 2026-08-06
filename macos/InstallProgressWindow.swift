import Cocoa

final class InstallProgressWindowController: NSWindowController, NSWindowDelegate {
    /// Holds failure-state controllers so the closable error window is not deallocated.
    fileprivate static var retainedControllers: [InstallProgressWindowController] = []

    private let phaseLabel = NSTextField(labelWithString: "准备中...")
    private let detailLabel = NSTextField(labelWithString: "")
    private let progress = NSProgressIndicator()
    private let elapsedLabel = NSTextField(labelWithString: "")
    private let startedAt = Date()
    private var timer: Timer?
    private let stepsStack = NSStackView()
    private var phases: [String] = []
    private var currentPhaseIndex: Int?
    private var streamedPhaseCount = 0
    private var stepDots: [NSView] = []
    private var stepLabels: [NSTextField] = []
    private let successTitle: String
    private let failureTitle: String
    private var finished = false

    init(
        title: String = "正在安装到 Codex",
        detail: String = "",
        successTitle: String = "✓ 完成",
        failureTitle: String = "✗ 失败"
    ) {
        self.successTitle = successTitle
        self.failureTitle = failureTitle
        let content = NSView(frame: NSRect(x: 0, y: 0, width: 500, height: 170))
        let window = NSPanel(
            contentRect: content.frame,
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.title = title
        window.isReleasedWhenClosed = false
        window.contentView = content
        super.init(window: window)
        window.delegate = self

        phaseLabel.font = .boldSystemFont(ofSize: 16)
        detailLabel.stringValue = detail
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.font = .systemFont(ofSize: 12)
        detailLabel.maximumNumberOfLines = 3
        detailLabel.lineBreakMode = .byWordWrapping
        detailLabel.isHidden = detail.isEmpty
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
            detailLabel.widthAnchor.constraint(lessThanOrEqualToConstant: 300),
        ])
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        window?.center()
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        timer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.elapsedLabel.stringValue = String(
                format: "已用时 %.1fs",
                Date().timeIntervalSince(self.startedAt)
            )
        }
        if let timer {
            RunLoop.main.add(timer, forMode: .common)
        }
    }

    /// Advances to a known synthetic phase by exact label match.
    func update(phase: String) {
        let display = localizedProgressLabel(phase)
        phaseLabel.stringValue = display
        if !phases.isEmpty,
           let index = phases.firstIndex(of: display) ?? phases.firstIndex(of: phase)
        {
            currentPhaseIndex = index
            refreshStepIndicator()
        }
    }

    /// Advances the step list in arrival order for streamed `MIXIN_PROGRESS` lines.
    func advanceStreamedPhase(_ rawPhase: String) {
        let display = localizedProgressLabel(rawPhase)
        phaseLabel.stringValue = display
        guard !phases.isEmpty else { return }
        let nextIndex = min(streamedPhaseCount, phases.count - 1)
        streamedPhaseCount = min(streamedPhaseCount + 1, phases.count)
        currentPhaseIndex = nextIndex
        if phases.indices.contains(nextIndex) {
            stepLabels[nextIndex].stringValue = display
        }
        refreshStepIndicator()
    }

    func advance(to index: Int) {
        guard !phases.isEmpty else { return }
        let clamped = min(max(index, 0), phases.count - 1)
        currentPhaseIndex = clamped
        phaseLabel.stringValue = phases[clamped]
        refreshStepIndicator()
    }

    func setDeterminateProgress(fraction: Double) {
        if progress.isIndeterminate {
            progress.stopAnimation(nil)
            progress.isIndeterminate = false
            progress.minValue = 0
            progress.maxValue = 1
        }
        progress.doubleValue = min(max(fraction, 0), 1)
    }

    func finish() {
        guard !finished else { return }
        finished = true
        timer?.invalidate()
        timer = nil
        if progress.isIndeterminate {
            progress.stopAnimation(nil)
            progress.isIndeterminate = false
            progress.minValue = 0
            progress.maxValue = 1
        }
        progress.doubleValue = 1
        progress.isHidden = false
        phaseLabel.textColor = .mixinHealthy
        phaseLabel.stringValue = successTitle
        if !phases.isEmpty {
            currentPhaseIndex = phases.count - 1
            refreshStepIndicator()
        }
        // Strongly retain self for the delay; weak capture would drop the controller
        // as soon as OperationProgress returns and leave a stuck success window.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            self.close()
        }
    }

    func finishAndWait() async {
        finish()
        try? await Task.sleep(nanoseconds: 850_000_000)
    }

    func fail(message: String) {
        guard !finished else { return }
        finished = true
        timer?.invalidate()
        timer = nil
        progress.stopAnimation(nil)
        progress.isHidden = true
        phaseLabel.textColor = .mixinError
        phaseLabel.stringValue = failureTitle
        detailLabel.isHidden = false
        detailLabel.textColor = .mixinError
        detailLabel.stringValue = message
        window?.styleMask.insert(.closable)
        window?.delegate = self
        // Keep the controller alive until the user dismisses the failure window.
        if !InstallProgressWindowController.retainedControllers.contains(where: { $0 === self }) {
            InstallProgressWindowController.retainedControllers.append(self)
        }
    }

    func windowWillClose(_ notification: Notification) {
        InstallProgressWindowController.retainedControllers.removeAll { $0 === self }
    }

    func setPhases(_ phases: [String]) {
        self.phases = phases
        streamedPhaseCount = 0
        stepsStack.arrangedSubviews.forEach {
            stepsStack.removeArrangedSubview($0)
            $0.removeFromSuperview()
        }
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
        let stepHeight = max(CGFloat(phases.count) * 22, 80)
        window?.setContentSize(NSSize(width: 520, height: max(220, stepHeight + 100)))
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

/// Maps CLI `MIXIN_PROGRESS` bodies (Chinese or English) to Chinese UI labels.
func localizedProgressLabel(_ raw: String) -> String {
    let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    let mapping: [String: String] = [
        "检查本地配置与网关状态": "检查本地配置与网关状态",
        "Checking local config and gateway status": "检查本地配置与网关状态",
        "获取 Codex 配置模板": "获取 Codex 配置模板",
        "Fetching Codex config template": "获取 Codex 配置模板",
        "获取可用模型列表": "获取可用模型列表",
        "Fetching available models": "获取可用模型列表",
        "加载模型元数据": "加载模型元数据",
        "Loading model metadata": "加载模型元数据",
        "写入 Codex 配置和模型目录": "写入 Codex 配置和模型目录",
        "Writing Codex config and model catalog": "写入 Codex 配置和模型目录",
        "同步历史会话与 SQLite 状态": "同步历史会话与 SQLite 状态",
        "Syncing history sessions and SQLite state": "同步历史会话与 SQLite 状态",
        "校验安装结果": "校验安装结果",
        "Validating install result": "校验安装结果",
        "读取并锁定 Codex 配置": "读取并锁定 Codex 配置",
        "Reading and locking Codex config": "读取并锁定 Codex 配置",
        "恢复安装前配置与登录状态": "恢复安装前配置与登录状态",
        "Restoring pre-install config and login state": "恢复安装前配置与登录状态",
        "恢复历史会话与 SQLite 状态": "恢复历史会话与 SQLite 状态",
        "Restoring history sessions and SQLite state": "恢复历史会话与 SQLite 状态",
    ]
    return mapping[trimmed] ?? trimmed
}

final class OperationProgress {
    let window: InstallProgressWindowController

    @MainActor
    init(
        title: String,
        phases: [String] = [],
        detail: String = "",
        successTitle: String = "✓ 完成",
        failureTitle: String = "✗ 失败"
    ) {
        window = InstallProgressWindowController(
            title: title,
            detail: detail,
            successTitle: successTitle,
            failureTitle: failureTitle
        )
        if !phases.isEmpty {
            window.setPhases(phases)
            window.advance(to: 0)
        }
        window.present()
    }

    func update(phase: String) {
        onMain { self.window.update(phase: phase) }
    }

    func advance(to index: Int) {
        onMain { self.window.advance(to: index) }
    }

    func advanceStreamedPhase(_ rawPhase: String) {
        onMain { self.window.advanceStreamedPhase(rawPhase) }
    }

    func setDeterminateProgress(fraction: Double) {
        onMain { self.window.setDeterminateProgress(fraction: fraction) }
    }

    func finish() {
        onMain { self.window.finish() }
    }

    func finishAndWait() async {
        await MainActor.run {
            self.window.finish()
        }
        try? await Task.sleep(nanoseconds: 850_000_000)
    }

    func fail(message: String) {
        onMain { self.window.fail(message: message) }
    }

    private func onMain(_ body: @escaping () -> Void) {
        if Thread.isMainThread {
            body()
        } else {
            DispatchQueue.main.async(execute: body)
        }
    }
}

func runOperationProgress<T>(
    title: String,
    phases: [String] = [],
    detail: String = "",
    successTitle: String = "✓ 完成",
    failureTitle: String = "✗ 失败",
    showFailureAlert: Bool = false,
    failureAlertTitle: String? = nil,
    work: (OperationProgress) async throws -> T
) async rethrows -> T {
    let progress = await MainActor.run {
        OperationProgress(
            title: title,
            phases: phases,
            detail: detail,
            successTitle: successTitle,
            failureTitle: failureTitle
        )
    }
    do {
        let result = try await work(progress)
        // Wait until the success window has closed so callers do not race it
        // with a modal alert while the controller is already deallocated.
        await progress.finishAndWait()
        return result
    } catch {
        progress.fail(message: localizedErrorDescription(error))
        if showFailureAlert {
            showAlert(
                title: failureAlertTitle ?? title,
                message: String(describing: error)
            )
        }
        throw error
    }
}
