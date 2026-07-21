import Cocoa

struct LoginFormValues {
    let provider: String
    let apiKey: String
    let baseUrl: String
    let imageGenerationPath: String
    let gatewayKey: String
    let quotaUrl: String
    let quotaUsername: String
}

struct QuotaUsage {
    let used: Double
    let limit: Double?
    let remaining: Double?
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

func runLoginSettingsPanel(stored: [String: Any]) -> LoginFormValues? {
    let providerPopup = NSPopUpButton()
    let providers: [(title: String, id: String)] = [
        ("Custom", "custom"),
        ("Baidu OneAPI", "baidu-oneapi"),
        ("OpenRouter", "openrouter"),
        ("DeepSeek", "deepseek"),
    ]
    for provider in providers {
        providerPopup.addItem(withTitle: provider.title)
        providerPopup.lastItem?.representedObject = provider.id
    }
    let storedProvider = stored["provider_preset"] as? String ?? "custom"
    if let index = providers.firstIndex(where: { $0.id == storedProvider }) {
        providerPopup.selectItem(at: index)
    }
    providerPopup.translatesAutoresizingMaskIntoConstraints = false
    providerPopup.heightAnchor.constraint(equalToConstant: 28).isActive = true

    let apiKeyField = formTextField()
    apiKeyField.placeholderString = stored["upstream_api_key"] == nil ? "必填：上游服务 API Key" : "留空保留已保存密钥"
    let baseUrlField = formTextField()
    baseUrlField.placeholderString = "只填根地址，如 https://host；不要填 /v1/messages、/v1/chat/completions 或 /anthropic"
    baseUrlField.stringValue = stored["upstream_base_url"] as? String ?? defaultBaseURL(for: storedProvider)
    let imageGenerationPathField = formTextField()
    imageGenerationPathField.placeholderString = "可选：如 /v1/images/generations；留空时自定义模型也走官方生图"
    imageGenerationPathField.stringValue = stored["upstream_image_generation_path"] as? String ?? defaultImageGenerationPath(for: storedProvider)
    let gatewayKeyField = formTextField()
    gatewayKeyField.placeholderString = stored["gateway_api_key"] == nil ? "可选：保护本地 127.0.0.1 网关的访问密钥" : "留空保留已保存密钥"
    let quotaUrlField = formTextField()
    quotaUrlField.placeholderString = "可选：完整额度查询 URL；返回 JSON 中包含 used/limit 等字段"
    quotaUrlField.stringValue = stored["quota_url"] as? String ?? defaultQuotaURL(for: storedProvider, baseURL: baseUrlField.stringValue)
    let quotaUsernameField = formTextField()
    quotaUsernameField.placeholderString = "可选：需要 username 查询参数的接口才填写"
    quotaUsernameField.stringValue = stored["quota_username"] as? String ?? ""

    let providerTarget = ModalActionTarget {
        let provider = selectedProviderID(providerPopup)
        let presetBaseURL = defaultBaseURL(for: provider)
        if !presetBaseURL.isEmpty {
            baseUrlField.stringValue = presetBaseURL
        }
        let presetImagePath = defaultImageGenerationPath(for: provider)
        if !presetImagePath.isEmpty {
            imageGenerationPathField.stringValue = presetImagePath
        } else if provider != storedProvider {
            imageGenerationPathField.stringValue = ""
        }
        let presetQuotaURL = defaultQuotaURL(for: provider, baseURL: baseUrlField.stringValue)
        if !presetQuotaURL.isEmpty {
            quotaUrlField.stringValue = presetQuotaURL
        } else if provider != storedProvider {
            quotaUrlField.stringValue = ""
        }
    }
    providerPopup.target = providerTarget
    providerPopup.action = #selector(ModalActionTarget.run(_:))

    let contentView = NSView(frame: NSRect(x: 0, y: 0, width: 760, height: 480))
    let panel = NSPanel(
        contentRect: contentView.frame,
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = "设置 Codex Mixin"
    panel.contentView = contentView
    panel.isReleasedWhenClosed = false
    panel.center()

    let titleLabel = NSTextField(labelWithString: "设置供应商与密钥")
    titleLabel.font = .boldSystemFont(ofSize: 18)
    titleLabel.textColor = .labelColor

    let detailLabel = NSTextField(wrappingLabelWithString: "配置会保存到 ~/.codex-mixin/config.json。API 密钥留空会保留已有配置；上游地址只填写根地址，路径由供应商预设或网关补齐。")
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    detailLabel.translatesAutoresizingMaskIntoConstraints = false
    detailLabel.widthAnchor.constraint(equalToConstant: 660).isActive = true

    let tokenButton = NSButton(title: "打开密钥页面", target: nil, action: nil)
    tokenButton.bezelStyle = .inline
    tokenButton.image = menuItemImage("key")
    tokenButton.imagePosition = .imageLeading
    tokenButton.contentTintColor = .controlAccentColor
    tokenButton.setButtonType(.momentaryPushIn)
    tokenButton.translatesAutoresizingMaskIntoConstraints = false
    let tokenTarget = ModalActionTarget {
        let provider = selectedProviderID(providerPopup)
        let rawURL = defaultCredentialURL(for: provider, baseURL: baseUrlField.stringValue)
        guard let url = URL(string: rawURL), !rawURL.isEmpty else {
            showAlert(title: "缺少密钥页面", message: "Custom 模式没有内置密钥页面。可以填写上游根地址后再打开，或直接从服务商控制台复制 API Key。")
            return
        }
        NSWorkspace.shared.open(url)
    }
    tokenButton.target = tokenTarget
    tokenButton.action = #selector(ModalActionTarget.run(_:))

    let formStack = NSStackView(views: [
        labeledView("供应商", providerPopup),
        labeledView("API 密钥", apiKeyField),
        labeledView("上游地址", baseUrlField),
        labeledView("上游生图路径", imageGenerationPathField),
        labeledView("本地密钥", gatewayKeyField),
        labeledView("额度接口", quotaUrlField),
        labeledView("额度用户", quotaUsernameField),
    ])
    formStack.orientation = .vertical
    formStack.spacing = 10

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
        buttonRowContainer.widthAnchor.constraint(equalToConstant: 660),
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

    var values: LoginFormValues?
    let saveTarget = ModalActionTarget {
        values = LoginFormValues(
            provider: selectedProviderID(providerPopup),
            apiKey: apiKeyField.stringValue,
            baseUrl: baseUrlField.stringValue,
            imageGenerationPath: imageGenerationPathField.stringValue,
            gatewayKey: gatewayKeyField.stringValue,
            quotaUrl: quotaUrlField.stringValue,
            quotaUsername: quotaUsernameField.stringValue
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

func defaultBaseURL(for provider: String) -> String {
    switch provider {
    case "baidu-oneapi":
        return "https://oneapi-comate.baidu-int.com"
    case "openrouter":
        return "https://openrouter.ai/api"
    case "deepseek":
        return "https://api.deepseek.com"
    default:
        return ""
    }
}

func defaultQuotaURL(for provider: String, baseURL: String) -> String {
    let trimmed = baseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    switch provider {
    case "baidu-oneapi":
        return trimmed.isEmpty ? "" : "\(trimmed)/openapi/v3/user/quota"
    case "openrouter":
        return trimmed.isEmpty ? "" : "\(trimmed)/v1/credits"
    default:
        return ""
    }
}

func defaultImageGenerationPath(for provider: String) -> String {
    provider == "baidu-oneapi" ? "/v1/images/generations" : ""
}

func defaultCredentialURL(for provider: String, baseURL: String) -> String {
    switch provider {
    case "baidu-oneapi":
        return "https://oneapi-comate.baidu-int.com/token"
    case "openrouter":
        return "https://openrouter.ai/settings/keys"
    case "deepseek":
        return "https://platform.deepseek.com/api_keys"
    default:
        return baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
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

func loadStoredConfig() throws -> [String: Any] {
    let configURL = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".codex-mixin")
        .appendingPathComponent("config.json")
    guard FileManager.default.fileExists(atPath: configURL.path) else {
        return [:]
    }
    let data = try Data(contentsOf: configURL)
    guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw GatewayError.command("\(configURL.path) 不是 JSON 对象")
    }
    return object
}

func appendOptionalArg(_ args: inout [String], _ name: String, _ rawValue: String) {
    let value = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
    if !value.isEmpty {
        args.append(name)
        args.append(value)
    }
}
