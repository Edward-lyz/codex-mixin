import Cocoa

func runInstallCodexPanel() -> Bool {
    let contentView = NSView(frame: NSRect(x: 0, y: 0, width: 760, height: 330))
    let panel = NSPanel(
        contentRect: contentView.frame,
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = "安装到 Codex"
    panel.contentView = contentView
    panel.isReleasedWhenClosed = false
    panel.center()

    let titleLabel = NSTextField(labelWithString: "安装 Codex OAuth 代理")
    titleLabel.font = .boldSystemFont(ofSize: 18)
    titleLabel.textColor = .labelColor

    let detailLabel = NSTextField(wrappingLabelWithString: "会先备份当前 ~/.codex/config.toml，再注册独立的 codex-mixin provider，并把现有历史会话统一迁移到该 provider。官方 GPT 保留原名并走 Codex 官方路径；上游 GPT 重名时使用 provider 后缀（例如 -baidu-oneapi）。完成后需要重启 Codex App；CLI 需要开新会话。")
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    detailLabel.translatesAutoresizingMaskIntoConstraints = false
    detailLabel.widthAnchor.constraint(equalToConstant: 660).isActive = true

    let pathStack = NSStackView(views: [
        labeledView("Codex 配置", copyableTextField("~/.codex/config.toml")),
        labeledView("模型目录", copyableTextField("~/.codex/model-catalogs/mixin-models.json")),
        labeledView("Provider", copyableTextField("codex-mixin / requires_openai_auth")),
    ])
    pathStack.orientation = .vertical
    pathStack.spacing = 10

    let cancelButton = NSButton(title: "取消", target: nil, action: nil)
    cancelButton.bezelStyle = .rounded
    cancelButton.translatesAutoresizingMaskIntoConstraints = false
    cancelButton.widthAnchor.constraint(equalToConstant: 96).isActive = true
    let installButton = NSButton(title: "安装", target: nil, action: nil)
    installButton.bezelStyle = .rounded
    installButton.keyEquivalent = "\r"
    installButton.translatesAutoresizingMaskIntoConstraints = false
    installButton.widthAnchor.constraint(equalToConstant: 96).isActive = true

    let buttonRow = NSStackView(views: [cancelButton, installButton])
    buttonRow.orientation = .horizontal
    buttonRow.alignment = .centerY
    buttonRow.spacing = 12
    buttonRow.translatesAutoresizingMaskIntoConstraints = false

    let buttonRowContainer = NSView()
    buttonRowContainer.translatesAutoresizingMaskIntoConstraints = false
    buttonRowContainer.addSubview(buttonRow)
    NSLayoutConstraint.activate([
        buttonRowContainer.widthAnchor.constraint(equalToConstant: 660),
        buttonRowContainer.heightAnchor.constraint(equalToConstant: 34),
        buttonRow.trailingAnchor.constraint(equalTo: buttonRowContainer.trailingAnchor),
        buttonRow.centerYAnchor.constraint(equalTo: buttonRowContainer.centerYAnchor),
    ])

    let mainStack = NSStackView(views: [titleLabel, detailLabel, pathStack, buttonRowContainer])
    mainStack.orientation = .vertical
    mainStack.alignment = .leading
    mainStack.spacing = 16
    mainStack.translatesAutoresizingMaskIntoConstraints = false
    contentView.addSubview(mainStack)
    NSLayoutConstraint.activate([
        mainStack.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 32),
        mainStack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -32),
        mainStack.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 28),
    ])

    var confirmed = false
    let installTarget = ModalActionTarget {
        confirmed = true
        NSApp.stopModal(withCode: .OK)
    }
    let cancelTarget = ModalActionTarget {
        NSApp.stopModal(withCode: .cancel)
    }
    installButton.target = installTarget
    installButton.action = #selector(ModalActionTarget.run(_:))
    cancelButton.target = cancelTarget
    cancelButton.action = #selector(ModalActionTarget.run(_:))
    panel.standardWindowButton(.closeButton)?.target = cancelTarget
    panel.standardWindowButton(.closeButton)?.action = #selector(ModalActionTarget.run(_:))

    NSApp.activate(ignoringOtherApps: true)
    let response = NSApp.runModal(for: panel)
    panel.close()
    _ = installTarget
    _ = cancelTarget
    return response == .OK && confirmed
}

