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
            return "官方 GPT、自定义模型目录和 Web Search 能力探测已完成。请重启 Codex App；CLI 需要开新会话。"
        case .customModelsOnly:
            return "自定义模型目录和 Web Search 能力探测已完成，未读取 OpenAI 模型缓存，并已选择自定义默认模型。请重启 Codex App；CLI 需要开新会话。"
        }
    }
}

func runInstallCodexPanel() -> CodexInstallMode? {
    let contentView = NSView(frame: NSRect(x: 0, y: 0, width: 760, height: 440))
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

    let titleLabel = NSTextField(labelWithString: "选择 Codex 安装模式")
    titleLabel.font = .boldSystemFont(ofSize: 18)
    titleLabel.textColor = .labelColor

    let detailLabel = NSTextField(wrappingLabelWithString: "两种模式都会备份 ~/.codex/config.toml、注册独立的 codex-mixin provider，并迁移现有历史会话。请选择与你的账号情况一致的模式。")
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    detailLabel.translatesAutoresizingMaskIntoConstraints = false
    detailLabel.widthAnchor.constraint(equalToConstant: 660).isActive = true

    let oauthButton = NSButton(title: "我有 OpenAI / ChatGPT 账号（保留官方 GPT）", target: nil, action: nil)
    oauthButton.setButtonType(.radio)
    let oauthDetail = NSTextField(wrappingLabelWithString: "需要 Codex 已登录并生成 ~/.codex/models_cache.json；官方 GPT 继续走官方路径，自定义模型走本地网关。")
    oauthDetail.textColor = .secondaryLabelColor
    oauthDetail.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    oauthDetail.translatesAutoresizingMaskIntoConstraints = false
    oauthDetail.widthAnchor.constraint(equalToConstant: 626).isActive = true

    let customOnlyButton = NSButton(title: "我没有 OpenAI / ChatGPT 账号（仅使用自定义模型）", target: nil, action: nil)
    customOnlyButton.setButtonType(.radio)
    let customOnlyDetail = NSTextField(wrappingLabelWithString: "不需要也不会读取 models_cache.json；只安装上游模型，并自动选择一个自定义模型作为默认模型。")
    customOnlyDetail.textColor = .secondaryLabelColor
    customOnlyDetail.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    customOnlyDetail.translatesAutoresizingMaskIntoConstraints = false
    customOnlyDetail.widthAnchor.constraint(equalToConstant: 626).isActive = true

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

    let modelsCachePath = NSString(string: "~/.codex/models_cache.json").expandingTildeInPath
    var selectedMode: CodexInstallMode = FileManager.default.fileExists(atPath: modelsCachePath)
        ? .openAIAccount
        : .customModelsOnly
    let applySelection: (CodexInstallMode) -> Void = { mode in
        selectedMode = mode
        oauthButton.state = mode == .openAIAccount ? .on : .off
        customOnlyButton.state = mode == .customModelsOnly ? .on : .off
        providerField.stringValue = mode == .openAIAccount
            ? "codex-mixin / requires_openai_auth"
            : "codex-mixin / custom models only"
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
    applySelection(selectedMode)

    var confirmedMode: CodexInstallMode?
    let installTarget = ModalActionTarget {
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
