import Cocoa

enum CodexInstallMode: Equatable {
    case openAIAccount
    case customModelsOnly

    var commandArguments: [String] {
        switch self {
        case .openAIAccount:
            return ["install-codex", "--codex-oauth-proxy"]
        case .customModelsOnly:
            return ["install-codex", "--custom-only"]
        }
    }

    var completionMessage: String {
        switch self {
        case .openAIAccount:
            return "官方账号模式已安装：官方 GPT、插件、云任务和账户功能保持可用，自定义模型同时加入模型选择器。请重启 Codex App；CLI 需要开新会话。"
        case .customModelsOnly:
            return "仅自定义模型模式已安装：模型选择器会显示自定义模型；界面中的 amazon-bedrock 只是本地登录占位，不会连接 AWS。官方插件、云任务和账户功能不可用。以后登录官方账号时，请先“从 Codex 恢复”，再改装官方账号模式。"
        }
    }
}

func runInstallCodexPanel() -> CodexInstallMode? {
    let contentView = NSView(frame: NSRect(x: 0, y: 0, width: 820, height: 520))
    let panel = NSPanel(
        contentRect: contentView.frame,
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = "安装到 Codex"
    panel.contentView = contentView
    panel.isReleasedWhenClosed = false
    configureTransientModalPanel(panel)
    panel.center()

    let titleLabel = NSTextField(labelWithString: "选择 Codex 安装模式")
    titleLabel.font = .boldSystemFont(ofSize: 18)
    titleLabel.textColor = .labelColor

    let detailLabel = NSTextField(wrappingLabelWithString: "先按下面条件选择模式。安装会备份 ~/.codex/config.toml；仅自定义模式还会备份 auth.json，卸载时一并恢复。两种模式可以切换，但请先执行“从 Codex 恢复”，再重新安装。")
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    detailLabel.translatesAutoresizingMaskIntoConstraints = false
    detailLabel.widthAnchor.constraint(equalToConstant: 720).isActive = true

    let oauthButton = NSButton(title: "官方账号模式（推荐：已登录 Codex）", target: nil, action: nil)
    oauthButton.setButtonType(.radio)
    let oauthDetail = NSTextField(wrappingLabelWithString: "前提：先在官方 Codex App 登录并打开一次。保留官方认证、GPT、插件、云任务和账户功能；自定义模型经本地网关加入同一个模型选择器。若尚未登录，请先取消安装并去 Codex 登录。")
    oauthDetail.textColor = .secondaryLabelColor
    oauthDetail.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    oauthDetail.translatesAutoresizingMaskIntoConstraints = false
    oauthDetail.widthAnchor.constraint(equalToConstant: 686).isActive = true

    let customOnlyButton = NSButton(title: "仅自定义模型模式（没有官方登录也可用）", target: nil, action: nil)
    customOnlyButton.setButtonType(.radio)
    let customOnlyDetail = NSTextField(wrappingLabelWithString: "会备份并临时替换 ~/.codex/auth.json，用本地 Bedrock 形态的占位身份开启 Desktop 模型选择器。界面可能显示 amazon-bedrock，但请求只到本地网关，不连接 AWS。官方插件、云任务和账户功能不可用；以后要用官方账号时，请先恢复再改装官方模式。")
    customOnlyDetail.textColor = .secondaryLabelColor
    customOnlyDetail.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    customOnlyDetail.translatesAutoresizingMaskIntoConstraints = false
    customOnlyDetail.widthAnchor.constraint(equalToConstant: 686).isActive = true

    let oauthOption = NSStackView(views: [oauthButton, oauthDetail])
    oauthOption.orientation = .vertical
    oauthOption.alignment = .leading
    oauthOption.spacing = 3
    oauthOption.setCustomSpacing(8, after: oauthButton)
    oauthDetail.setContentHuggingPriority(.defaultLow, for: .horizontal)

    let customOnlyOption = NSStackView(views: [customOnlyButton, customOnlyDetail])
    customOnlyOption.orientation = .vertical
    customOnlyOption.alignment = .leading
    customOnlyOption.spacing = 3
    customOnlyOption.setCustomSpacing(8, after: customOnlyButton)
    customOnlyDetail.setContentHuggingPriority(.defaultLow, for: .horizontal)

    let modeStack = NSStackView(views: [oauthOption, customOnlyOption])
    modeStack.orientation = .vertical
    modeStack.alignment = .leading
    modeStack.spacing = 12

    let providerField = copyableTextField("")
    let pathStack = NSStackView(views: [
        labeledView("Codex 配置", copyableTextField("~/.codex/config.toml")),
        labeledView("模型目录", copyableTextField("~/.codex/model-catalogs/mixin-models.json")),
        labeledView("Provider", providerField),
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
    installButton.isEnabled = false
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
        buttonRowContainer.widthAnchor.constraint(equalToConstant: 720),
        buttonRowContainer.heightAnchor.constraint(equalToConstant: 34),
        buttonRow.trailingAnchor.constraint(equalTo: buttonRowContainer.trailingAnchor),
        buttonRow.centerYAnchor.constraint(equalTo: buttonRowContainer.centerYAnchor),
    ])

    let mainStack = NSStackView(views: [titleLabel, detailLabel, modeStack, pathStack, buttonRowContainer])
    mainStack.orientation = .vertical
    mainStack.alignment = .leading
    mainStack.spacing = 14
    mainStack.translatesAutoresizingMaskIntoConstraints = false
    contentView.addSubview(mainStack)
    NSLayoutConstraint.activate([
        mainStack.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 32),
        mainStack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -32),
        mainStack.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 28),
    ])

    var selectedMode: CodexInstallMode?
    let applySelection: (CodexInstallMode) -> Void = { mode in
        selectedMode = mode
        oauthButton.state = mode == .openAIAccount ? .on : .off
        customOnlyButton.state = mode == .customModelsOnly ? .on : .off
        installButton.isEnabled = true
        providerField.stringValue = mode == .openAIAccount
            ? "codex-mixin / 官方 OAuth 代理"
            : "amazon-bedrock / 本地占位（不连接 AWS）"
    }
    let oauthTarget = ModalActionTarget {
        applySelection(.openAIAccount)
    }
    let customOnlyTarget = ModalActionTarget {
        applySelection(.customModelsOnly)
    }
    oauthButton.target = oauthTarget
    oauthButton.action = #selector(ModalActionTarget.run(_:))
    customOnlyButton.target = customOnlyTarget
    customOnlyButton.action = #selector(ModalActionTarget.run(_:))
    var confirmedMode: CodexInstallMode?
    let installTarget = ModalActionTarget {
        guard let selectedMode else { return }
        confirmedMode = selectedMode
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
    _ = oauthTarget
    _ = customOnlyTarget
    _ = installTarget
    _ = cancelTarget
    return response == .OK ? confirmedMode : nil
}
