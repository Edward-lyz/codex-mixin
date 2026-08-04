import Cocoa

final class InstallProgressWindowController: NSWindowController {
    private let phaseLabel = NSTextField(labelWithString: "准备中...")
    private let detailLabel = NSTextField(labelWithString: "安装期间请勿打开 Codex App")
    private let progress = NSProgressIndicator()
    private let elapsedLabel = NSTextField(labelWithString: "")
    private let startedAt = Date()
    private var timer: Timer?

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

        let stack = NSStackView(views: [phaseLabel, progress, detailLabel, elapsedLabel])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 28),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -28),
            stack.centerYAnchor.constraint(equalTo: content.centerYAnchor),
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
    }

    func finish() {
        timer?.invalidate()
        timer = nil
        progress.stopAnimation(nil)
        close()
    }
}
