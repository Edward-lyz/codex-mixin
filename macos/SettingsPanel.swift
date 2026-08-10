import Cocoa

struct AddProviderFormValues {
    let preset: String
    let displayName: String
    let baseURL: String
    let websiteURL: String
    let apiKey: String
    let quotaUsername: String
    let quotaWorkspaceID: String
    let quotaAuthCookie: String
    let baiduAuthBridge: String
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

func runAddProviderSheet(
    attachedTo parentWindow: NSWindow,
    completion: @escaping (AddProviderFormValues?) -> Void
) {
    let providerPopup = NSPopUpButton()
    let providers: [(title: String, id: String)] = [
        ("Baidu OneAPI", "baidu-oneapi"),
        ("OpenRouter", "openrouter"),
        ("DeepSeek", "deepseek"),
        ("OpenCode Go", "opencode-go"),
        (AppLocalization.string("settings.customSite"), "custom"),
    ]
    for provider in providers {
        providerPopup.addItem(withTitle: provider.title)
        providerPopup.lastItem?.representedObject = provider.id
    }
    providerPopup.translatesAutoresizingMaskIntoConstraints = false
    providerPopup.heightAnchor.constraint(equalToConstant: 28).isActive = true

    let apiKeyField = secureFormTextField()
    apiKeyField.placeholderString = AppLocalization.string("settings.requiredStoredOnlyByTheLocalRust")
    let quotaUsernameField = formTextField()
    quotaUsernameField.placeholderString = AppLocalization.string("settings.baiduOneAPIQuotaUsername")
    let quotaUsernameRow = labeledView(
        AppLocalization.string("settings.quotaUsername"),
        quotaUsernameField
    )
    let quotaWorkspaceIDField = formTextField()
    quotaWorkspaceIDField.placeholderString = AppLocalization.string("settings.forExampleWrkAbc123")
    let quotaWorkspaceIDRow = labeledView(
        AppLocalization.string("settings.workspaceID"),
        quotaWorkspaceIDField
    )
    let quotaAuthCookieField = secureFormTextField()
    quotaAuthCookieField.placeholderString = AppLocalization.string("settings.opencodeAiAuthCookie")
    let quotaAuthCookieRow = labeledView(
        AppLocalization.string("settings.authCookie"),
        quotaAuthCookieField
    )
    let displayNameField = formTextField()
    displayNameField.placeholderString = AppLocalization.string("settings.forExampleCommunityAPI")
    let displayNameRow = labeledView(
        AppLocalization.string("settings.siteName"),
        displayNameField
    )
    let baseURLField = formTextField()
    baseURLField.placeholderString = "https://example.com/v1"
    let baseURLRow = labeledView(
        AppLocalization.string("settings.apiURL"),
        baseURLField
    )
    let websiteURLField = formTextField()
    websiteURLField.placeholderString = "https://example.com"
    let websiteURLRow = labeledView("官网地址", websiteURLField)
    let baiduAuthBridgePopup = baiduAuthBridgePopUpButton()
    let baiduAuthBridgeRow = labeledView(
        AppLocalization.string("settings.authBridge"),
        baiduAuthBridgePopup
    )

    let contentView = NSView(frame: NSRect(x: 0, y: 0, width: 650, height: 700))
    let panel = NSWindow(
        contentRect: contentView.frame,
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = AppLocalization.string("settings.addProvider")
    panel.contentView = contentView
    panel.isReleasedWhenClosed = false

    let titleLabel = NSTextField(
        labelWithString: AppLocalization.string("settings.addProvider2")
    )
    titleLabel.font = .boldSystemFont(ofSize: 18)

    let detailLabel = NSTextField(wrappingLabelWithString: AppLocalization.string("settings.chooseASubscriptionAndEnterItsCredentials"))
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    detailLabel.translatesAutoresizingMaskIntoConstraints = false
    detailLabel.widthAnchor.constraint(equalToConstant: 550).isActive = true

    let tokenButton = NSButton(
        title: AppLocalization.string("settings.openAPIKeyPage"),
        target: nil,
        action: nil
    )
    tokenButton.bezelStyle = .inline
    tokenButton.image = menuItemImage("key")
    tokenButton.imagePosition = .imageLeading
    tokenButton.contentTintColor = .controlAccentColor
    let tokenTarget = ModalActionTarget {
        guard let url = URL(string: providerCredentialURL(selectedProviderID(providerPopup))) else {
            return
        }
        NSWorkspace.shared.open(url)
    }
    tokenButton.target = tokenTarget
    tokenButton.action = #selector(ModalActionTarget.run(_:))

    let providerTarget = ModalActionTarget {
        let provider = selectedProviderID(providerPopup)
        let isCustom = provider == "custom"
        quotaUsernameRow.isHidden = provider != "baidu-oneapi"
        quotaWorkspaceIDRow.isHidden = !requiresOpenCodeGoQuotaCredentials(provider)
        quotaAuthCookieRow.isHidden = !requiresOpenCodeGoQuotaCredentials(provider)
        displayNameRow.isHidden = !isCustom
        baseURLRow.isHidden = !isCustom
        websiteURLRow.isHidden = !isCustom
        baiduAuthBridgeRow.isHidden = provider != "baidu-oneapi"
        tokenButton.isHidden = isCustom
    }
    providerPopup.target = providerTarget
    providerPopup.action = #selector(ModalActionTarget.run(_:))
    providerTarget.run(nil)

    let formStack = NSStackView(views: [
        labeledView(AppLocalization.string("settings.provider"), providerPopup),
        displayNameRow,
        baseURLRow,
        websiteURLRow,
        labeledView("API Key", apiKeyField),
        quotaUsernameRow,
        quotaWorkspaceIDRow,
        quotaAuthCookieRow,
        baiduAuthBridgeRow,
    ])
    formStack.orientation = .vertical
    formStack.spacing = 10

    let cancelButton = NSButton(
        title: AppLocalization.string("settings.cancel"),
        target: nil,
        action: nil
    )
    cancelButton.bezelStyle = .rounded
    let saveButton = NSButton(
        title: AppLocalization.string("settings.add"),
        target: nil,
        action: nil
    )
    saveButton.bezelStyle = .rounded
    saveButton.keyEquivalent = "\r"
    let buttonRow = NSStackView(views: [cancelButton, saveButton])
    buttonRow.orientation = .horizontal
    buttonRow.spacing = 12

    let mainStack = NSStackView(views: [
        titleLabel,
        detailLabel,
        tokenButton,
        formStack,
        buttonRow,
    ])
    mainStack.orientation = .vertical
    mainStack.alignment = .leading
    mainStack.spacing = 18
    mainStack.translatesAutoresizingMaskIntoConstraints = false
    contentView.addSubview(mainStack)
    NSLayoutConstraint.activate([
        mainStack.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 36),
        mainStack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -36),
        mainStack.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 30),
        buttonRow.trailingAnchor.constraint(equalTo: mainStack.trailingAnchor),
    ])

    var values: AddProviderFormValues?
    let saveTarget = ModalActionTarget {
        let preset = selectedProviderID(providerPopup)
        let displayName = displayNameField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let baseURL = baseURLField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if preset == "custom", displayName.isEmpty || baseURL.isEmpty {
            showAlert(
                title: AppLocalization.string("settings.customSiteInformationRequired"),
                message: AppLocalization.string("settings.enterTheSiteNameAndAPIURL")
            )
            return
        }
        let apiKey = apiKeyField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !apiKey.isEmpty else {
            showAlert(
                title: "缺少 API 密钥",
                message: AppLocalization.string("settings.enterTheProviderAPIKey")
            )
            return
        }
        let username = quotaUsernameField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if preset == "baidu-oneapi", username.isEmpty {
            showAlert(
                title: "缺少额度用户名",
                message: AppLocalization.string("settings.enterTheBaiduOneAPIQuotaUsername")
            )
            return
        }
        let workspaceID = quotaWorkspaceIDField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let authCookie = quotaAuthCookieField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if requiresOpenCodeGoQuotaCredentials(preset), workspaceID.isEmpty || authCookie.isEmpty {
            showAlert(
                title: AppLocalization.string("settings.opencodeGoQuotaCredentialsRequired"),
                message: AppLocalization.string("settings.opencodeGoRequiresBothTheWorkspaceID")
            )
            return
        }
        values = AddProviderFormValues(
            preset: preset,
            displayName: displayName,
            baseURL: baseURL,
            websiteURL: websiteURLField.stringValue
                .trimmingCharacters(in: .whitespacesAndNewlines),
            apiKey: apiKey,
            quotaUsername: username,
            quotaWorkspaceID: workspaceID,
            quotaAuthCookie: authCookie,
            baiduAuthBridge: selectedPopupValue(
                baiduAuthBridgePopup,
                fallback: "disabled"
            )
        )
        parentWindow.endSheet(panel, returnCode: .OK)
    }
    let cancelTarget = ModalActionTarget {
        parentWindow.endSheet(panel, returnCode: .cancel)
    }
    saveButton.target = saveTarget
    saveButton.action = #selector(ModalActionTarget.run(_:))
    cancelButton.target = cancelTarget
    cancelButton.action = #selector(ModalActionTarget.run(_:))
    panel.standardWindowButton(.closeButton)?.target = cancelTarget
    panel.standardWindowButton(.closeButton)?.action = #selector(ModalActionTarget.run(_:))

    NSApp.activate(ignoringOtherApps: true)
    let actionTargets = [tokenTarget, providerTarget, saveTarget, cancelTarget]
    parentWindow.beginSheet(panel) { response in
        panel.close()
        _ = actionTargets
        completion(response == .OK ? values : nil)
    }
}

func baiduAuthBridgePopUpButton() -> NSPopUpButton {
    let popup = NSPopUpButton()
    let modes: [(String, String)] = [
        (AppLocalization.string("settings.disabledDefault"), "disabled"),
        (
            AppLocalization.string("settings.duccCoreLoopback"),
            "ducc_loopback"
        ),
    ]
    for (title, value) in modes {
        popup.addItem(withTitle: title)
        popup.lastItem?.representedObject = value
    }
    popup.toolTip = AppLocalization.string("settings.duccUsesACodexMixinManagedCopy")
    popup.translatesAutoresizingMaskIntoConstraints = false
    popup.heightAnchor.constraint(equalToConstant: 28).isActive = true
    selectPopupValue(popup, "disabled")
    return popup
}

func configureTransientModalPanel(_ panel: NSPanel) {
    panel.level = .normal
    panel.isFloatingPanel = false
    panel.hidesOnDeactivate = true
    panel.becomesKeyOnlyIfNeeded = false
}

func selectedProviderID(_ popup: NSPopUpButton) -> String {
    popup.selectedItem?.representedObject as? String ?? "baidu-oneapi"
}

func requiresOpenCodeGoQuotaCredentials(_ provider: String) -> Bool {
    provider == "opencode-go"
}

func providerCredentialURL(_ provider: String) -> String {
    switch provider {
    case "baidu-oneapi": return "https://oneapi-comate.baidu-int.com/token"
    case "openrouter": return "https://openrouter.ai/settings/keys"
    case "deepseek": return "https://platform.deepseek.com/api_keys"
    case "opencode-go": return "https://opencode.ai/go"
    default: return ""
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
    // 110pt label 宽度，用于设置 sheet / 独立 modal 面板中的宽松表单。
    let label = NSTextField(labelWithString: title)
    label.alignment = .right
    label.textColor = .secondaryLabelColor
    label.translatesAutoresizingMaskIntoConstraints = false
    label.widthAnchor.constraint(equalToConstant: 110).isActive = true
    field.translatesAutoresizingMaskIntoConstraints = false
    field.widthAnchor.constraint(equalToConstant: 420).isActive = true
    let row = NSStackView(views: [label, field])
    row.orientation = .horizontal
    row.alignment = .centerY
    row.spacing = 10
    return row
}

func formTextField() -> NSTextField {
    configuredFormTextField(NSTextField())
}

func secureFormTextField() -> NSSecureTextField {
    configuredFormTextField(NSSecureTextField())
}

private func configuredFormTextField<T: NSTextField>(_ field: T) -> T {
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
