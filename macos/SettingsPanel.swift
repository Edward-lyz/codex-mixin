import Cocoa

struct AddProviderFormValues {
    let preset: String
    let id: String
    let displayName: String
    let apiKey: String
    let baseUrl: String
    let protocolID: String
    let apiPath: String
    let modelsPath: String
    let imageGenerationPath: String
    let gatewayKey: String
    let quotaUrl: String
    let quotaUsername: String
    let quotaCurrency: String
    let quotaParser: String
}

struct ProviderPresetFormDefaults {
    let id: String
    let displayName: String
    let baseURL: String
    let protocolID: String
    let apiPath: String
    let modelsPath: String
    let imageGenerationPath: String
    let quotaURL: String
    let quotaCurrency: String
    let quotaParser: String
    let credentialURL: String
}

final class ModalActionTarget: NSObject {
    let action: () -> Void

    init(_ action: @escaping () -> Void) {
        self.action = action
    }

    @objc func run(_ sender: Any?) {
        action()
    }
}

func runAddProviderPanel(gatewayAuthConfigured: Bool) -> AddProviderFormValues? {
    let providerPopup = NSPopUpButton()
    let providers: [(title: String, id: String)] = [
        ("Custom", "custom"),
        ("Baidu OneAPI", "baidu-oneapi"),
        ("OpenRouter", "openrouter"),
        ("DeepSeek", "deepseek"),
        ("OpenCode Go", "opencode-go"),
    ]
    for provider in providers {
        providerPopup.addItem(withTitle: provider.title)
        providerPopup.lastItem?.representedObject = provider.id
    }
    providerPopup.translatesAutoresizingMaskIntoConstraints = false
    providerPopup.heightAnchor.constraint(equalToConstant: 28).isActive = true

    let idField = formTextField()
    idField.placeholderString = "小写字母、数字、点、下划线或短横线；会作为模型后缀"
    let displayNameField = formTextField()
    displayNameField.placeholderString = "在设置页和测速页显示的名称"
    let apiKeyField = formTextField()
    apiKeyField.placeholderString = "必填：只交给 Rust CLI 保存，Swift 不读取已保存密钥"
    let baseUrlField = formTextField()
    baseUrlField.placeholderString = "根地址，如 https://host"
    let protocolPopup = NSPopUpButton()
    for protocolOption in [
        ("Anthropic Messages", "anthropic_messages"),
        ("OpenAI Chat Completions", "open_ai_chat"),
        ("OpenAI Responses", "open_ai_responses"),
    ] {
        protocolPopup.addItem(withTitle: protocolOption.0)
        protocolPopup.lastItem?.representedObject = protocolOption.1
    }
    protocolPopup.translatesAutoresizingMaskIntoConstraints = false
    protocolPopup.heightAnchor.constraint(equalToConstant: 28).isActive = true
    let apiPathField = formTextField()
    let modelsPathField = formTextField()
    let imageGenerationPathField = formTextField()
    imageGenerationPathField.placeholderString = "可选，如 /v1/images/generations"
    let gatewayKeyField = formTextField()
    gatewayKeyField.placeholderString = gatewayAuthConfigured
        ? "本地网关密钥已配置；留空保留"
        : "可选：保护本地 127.0.0.1 网关"
    let quotaUrlField = formTextField()
    quotaUrlField.placeholderString = "可选：完整额度查询 URL"
    let quotaUsernameField = formTextField()
    quotaUsernameField.placeholderString = "Baidu OneAPI 必填"
    let quotaCurrencyField = formTextField()
    quotaCurrencyField.placeholderString = "可选，三位币种代码，如 USD/CNY"
    let quotaParserPopup = NSPopUpButton()
    for parser in [
        ("Generic", "generic"),
        ("Baidu OneAPI", "baidu_oneapi"),
        ("OpenRouter", "openrouter"),
    ] {
        quotaParserPopup.addItem(withTitle: parser.0)
        quotaParserPopup.lastItem?.representedObject = parser.1
    }

    let providerTarget = ModalActionTarget {
        let provider = selectedProviderID(providerPopup)
        let defaults = providerFormDefaults(provider)
        idField.stringValue = defaults.id
        displayNameField.stringValue = defaults.displayName
        baseUrlField.stringValue = defaults.baseURL
        selectPopupValue(protocolPopup, defaults.protocolID)
        apiPathField.stringValue = defaults.apiPath
        modelsPathField.stringValue = defaults.modelsPath
        imageGenerationPathField.stringValue = defaults.imageGenerationPath
        quotaUrlField.stringValue = defaults.quotaURL
        quotaCurrencyField.stringValue = defaults.quotaCurrency
        selectPopupValue(quotaParserPopup, defaults.quotaParser)
    }
    providerPopup.target = providerTarget
    providerPopup.action = #selector(ModalActionTarget.run(_:))
    providerTarget.run(nil)

    let contentView = NSView(frame: NSRect(x: 0, y: 0, width: 780, height: 720))
    let panel = NSPanel(
        contentRect: contentView.frame,
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = "新增供应商"
    panel.contentView = contentView
    panel.isReleasedWhenClosed = false
    panel.center()

    let titleLabel = NSTextField(labelWithString: "新增供应商")
    titleLabel.font = .boldSystemFont(ofSize: 18)
    titleLabel.textColor = .labelColor

    let detailLabel = NSTextField(wrappingLabelWithString: "模型会以 <上游模型 ID>-<供应商 ID> 出现在 Codex。保存通过 Rust CLI 完成文件锁、校验和原子替换；窗口不会读取配置文件中的明文密钥。")
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    detailLabel.translatesAutoresizingMaskIntoConstraints = false
    detailLabel.widthAnchor.constraint(equalToConstant: 680).isActive = true

    let tokenButton = NSButton(title: "打开密钥页面", target: nil, action: nil)
    tokenButton.bezelStyle = .inline
    tokenButton.image = menuItemImage("key")
    tokenButton.imagePosition = .imageLeading
    tokenButton.contentTintColor = .controlAccentColor
    tokenButton.setButtonType(.momentaryPushIn)
    tokenButton.translatesAutoresizingMaskIntoConstraints = false
    let tokenTarget = ModalActionTarget {
        let provider = selectedProviderID(providerPopup)
        let presetURL = providerFormDefaults(provider).credentialURL
        let rawURL = presetURL.isEmpty ? baseUrlField.stringValue : presetURL
        guard let url = URL(string: rawURL), !rawURL.isEmpty else {
            showAlert(title: "缺少密钥页面", message: "Custom 预设没有内置密钥页面。请从服务商控制台复制 API Key。")
            return
        }
        NSWorkspace.shared.open(url)
    }
    tokenButton.target = tokenTarget
    tokenButton.action = #selector(ModalActionTarget.run(_:))

    let formStack = NSStackView(views: [
        labeledView("预设", providerPopup),
        labeledView("供应商 ID", idField),
        labeledView("显示名称", displayNameField),
        labeledView("API 密钥", apiKeyField),
        labeledView("上游根地址", baseUrlField),
        labeledView("协议", protocolPopup),
        labeledView("推理路径", apiPathField),
        labeledView("模型路径", modelsPathField),
        labeledView("生图路径", imageGenerationPathField),
        labeledView("本地密钥", gatewayKeyField),
        labeledView("额度接口", quotaUrlField),
        labeledView("额度用户名 *", quotaUsernameField),
        labeledView("额度币种", quotaCurrencyField),
        labeledView("额度解析器", quotaParserPopup),
    ])
    formStack.orientation = .vertical
    formStack.spacing = 7

    let cancelButton = NSButton(title: "取消", target: nil, action: nil)
    cancelButton.bezelStyle = .rounded
    cancelButton.translatesAutoresizingMaskIntoConstraints = false
    cancelButton.widthAnchor.constraint(equalToConstant: 96).isActive = true
    let saveButton = NSButton(title: "保存", target: nil, action: nil)
    saveButton.bezelStyle = .rounded
    saveButton.keyEquivalent = "\r"
    saveButton.translatesAutoresizingMaskIntoConstraints = false
    saveButton.widthAnchor.constraint(equalToConstant: 96).isActive = true
    let buttonRow = NSStackView(views: [cancelButton, saveButton])
    buttonRow.orientation = .horizontal
    buttonRow.alignment = .centerY
    buttonRow.spacing = 12
    buttonRow.translatesAutoresizingMaskIntoConstraints = false

    let buttonRowContainer = NSView()
    buttonRowContainer.translatesAutoresizingMaskIntoConstraints = false
    buttonRowContainer.addSubview(buttonRow)
    NSLayoutConstraint.activate([
        buttonRowContainer.widthAnchor.constraint(equalToConstant: 680),
        buttonRowContainer.heightAnchor.constraint(equalToConstant: 34),
        buttonRow.trailingAnchor.constraint(equalTo: buttonRowContainer.trailingAnchor),
        buttonRow.centerYAnchor.constraint(equalTo: buttonRowContainer.centerYAnchor),
    ])

    let mainStack = NSStackView(views: [titleLabel, detailLabel, tokenButton, formStack, buttonRowContainer])
    mainStack.orientation = .vertical
    mainStack.alignment = .leading
    mainStack.spacing = 16
    mainStack.translatesAutoresizingMaskIntoConstraints = false
    contentView.addSubview(mainStack)
    NSLayoutConstraint.activate([
        mainStack.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 32),
        mainStack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -32),
        mainStack.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 28),
        buttonRow.trailingAnchor.constraint(equalTo: mainStack.trailingAnchor),
    ])

    var values: AddProviderFormValues?
    let saveTarget = ModalActionTarget {
        let preset = selectedProviderID(providerPopup)
        if preset == "baidu-oneapi",
           quotaUsernameField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        {
            showAlert(
                title: "缺少额度用户名",
                message: "Baidu OneAPI 查询额度必须填写用户名。"
            )
            return
        }
        values = AddProviderFormValues(
            preset: preset,
            id: idField.stringValue,
            displayName: displayNameField.stringValue,
            apiKey: apiKeyField.stringValue,
            baseUrl: baseUrlField.stringValue,
            protocolID: selectedPopupValue(protocolPopup, fallback: "anthropic_messages"),
            apiPath: apiPathField.stringValue,
            modelsPath: modelsPathField.stringValue,
            imageGenerationPath: imageGenerationPathField.stringValue,
            gatewayKey: gatewayKeyField.stringValue,
            quotaUrl: quotaUrlField.stringValue,
            quotaUsername: quotaUsernameField.stringValue,
            quotaCurrency: quotaCurrencyField.stringValue,
            quotaParser: selectedPopupValue(quotaParserPopup, fallback: "generic")
        )
        NSApp.stopModal(withCode: .OK)
    }
    let cancelTarget = ModalActionTarget {
        NSApp.stopModal(withCode: .cancel)
    }
    saveButton.target = saveTarget
    saveButton.action = #selector(ModalActionTarget.run(_:))
    cancelButton.target = cancelTarget
    cancelButton.action = #selector(ModalActionTarget.run(_:))
    panel.standardWindowButton(.closeButton)?.target = cancelTarget
    panel.standardWindowButton(.closeButton)?.action = #selector(ModalActionTarget.run(_:))

    NSApp.activate(ignoringOtherApps: true)
    let response = NSApp.runModal(for: panel)
    panel.close()
    _ = tokenTarget
    _ = providerTarget
    _ = saveTarget
    _ = cancelTarget
    return response == .OK ? values : nil
}

func selectedProviderID(_ popup: NSPopUpButton) -> String {
    popup.selectedItem?.representedObject as? String ?? "custom"
}

func providerFormDefaults(_ provider: String) -> ProviderPresetFormDefaults {
    switch provider {
    case "baidu-oneapi":
        return ProviderPresetFormDefaults(
            id: provider,
            displayName: "Baidu OneAPI",
            baseURL: "https://oneapi-comate.baidu-int.com",
            protocolID: "anthropic_messages",
            apiPath: "/v1/messages",
            modelsPath: "",
            imageGenerationPath: "/v1/images/generations",
            quotaURL: "https://oneapi-comate.baidu-int.com/openapi/v3/user/quota",
            quotaCurrency: "CNY",
            quotaParser: "baidu_oneapi",
            credentialURL: "https://oneapi-comate.baidu-int.com/token"
        )
    case "openrouter":
        return ProviderPresetFormDefaults(
            id: provider,
            displayName: "OpenRouter",
            baseURL: "https://openrouter.ai/api",
            protocolID: "open_ai_chat",
            apiPath: "/v1/chat/completions",
            modelsPath: "/v1/models",
            imageGenerationPath: "",
            quotaURL: "https://openrouter.ai/api/v1/credits",
            quotaCurrency: "USD",
            quotaParser: "openrouter",
            credentialURL: "https://openrouter.ai/settings/keys"
        )
    case "deepseek":
        return ProviderPresetFormDefaults(
            id: provider,
            displayName: "DeepSeek",
            baseURL: "https://api.deepseek.com",
            protocolID: "open_ai_chat",
            apiPath: "/chat/completions",
            modelsPath: "/models",
            imageGenerationPath: "",
            quotaURL: "",
            quotaCurrency: "",
            quotaParser: "generic",
            credentialURL: "https://platform.deepseek.com/api_keys"
        )
    case "opencode-go":
        return ProviderPresetFormDefaults(
            id: provider,
            displayName: "OpenCode Go",
            baseURL: "https://opencode.ai/zen/go",
            protocolID: "open_ai_chat",
            apiPath: "/v1/chat/completions",
            modelsPath: "/v1/models",
            imageGenerationPath: "",
            quotaURL: "",
            quotaCurrency: "",
            quotaParser: "generic",
            credentialURL: "https://opencode.ai/go"
        )
    default:
        return ProviderPresetFormDefaults(
            id: "custom",
            displayName: "Custom",
            baseURL: "",
            protocolID: "anthropic_messages",
            apiPath: "/v1/messages",
            modelsPath: "/v1/models",
            imageGenerationPath: "",
            quotaURL: "",
            quotaCurrency: "",
            quotaParser: "generic",
            credentialURL: ""
        )
    }
}

func selectedPopupValue(_ popup: NSPopUpButton, fallback: String) -> String {
    popup.selectedItem?.representedObject as? String ?? fallback
}

func selectPopupValue(_ popup: NSPopUpButton, _ value: String) {
    if let item = popup.itemArray.first(where: { ($0.representedObject as? String) == value }) {
        popup.select(item)
    }
}


func labeledView(_ title: String, _ field: NSView) -> NSView {
    let label = NSTextField(labelWithString: title)
    label.alignment = .right
    label.textColor = .secondaryLabelColor
    label.translatesAutoresizingMaskIntoConstraints = false
    label.widthAnchor.constraint(equalToConstant: 110).isActive = true
    field.translatesAutoresizingMaskIntoConstraints = false
    field.widthAnchor.constraint(equalToConstant: 540).isActive = true
    let row = NSStackView(views: [label, field])
    row.orientation = .horizontal
    row.alignment = .centerY
    row.spacing = 10
    return row
}

func formTextField() -> NSTextField {
    let field = NSTextField()
    field.controlSize = .regular
    field.font = .systemFont(ofSize: NSFont.systemFontSize)
    field.lineBreakMode = .byTruncatingMiddle
    field.translatesAutoresizingMaskIntoConstraints = false
    field.heightAnchor.constraint(equalToConstant: 28).isActive = true
    return field
}

func copyableTextField(_ value: String) -> NSTextField {
    let field = NSTextField()
    field.stringValue = value
    field.isEditable = false
    field.isSelectable = true
    field.isBordered = false
    field.drawsBackground = false
    field.font = .systemFont(ofSize: NSFont.systemFontSize)
    field.textColor = .labelColor
    field.lineBreakMode = .byTruncatingMiddle
    field.translatesAutoresizingMaskIntoConstraints = false
    field.heightAnchor.constraint(equalToConstant: 28).isActive = true
    return field
}
