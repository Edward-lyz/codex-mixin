import Cocoa

struct AddProviderFormValues {
    let preset: String
    let displayName: String
    let baseURL: String
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
        (appText("自定义站点", "自訂站點", "Custom site"), "custom"),
    ]
    for provider in providers {
        providerPopup.addItem(withTitle: provider.title)
        providerPopup.lastItem?.representedObject = provider.id
    }
    providerPopup.translatesAutoresizingMaskIntoConstraints = false
    providerPopup.heightAnchor.constraint(equalToConstant: 28).isActive = true

    let apiKeyField = secureFormTextField()
    apiKeyField.placeholderString = appText(
        "必填；密钥只交给本地 Rust CLI 保存",
        "必填；金鑰只交給本機 Rust CLI 儲存",
        "Required; stored only by the local Rust CLI"
    )
    let quotaUsernameField = formTextField()
    quotaUsernameField.placeholderString = appText(
        "Baidu OneAPI 额度查询用户名",
        "Baidu OneAPI 額度查詢使用者名稱",
        "Baidu OneAPI quota username"
    )
    let quotaUsernameRow = labeledView(
        appText("额度用户名", "額度使用者名稱", "Quota username"),
        quotaUsernameField
    )
    let quotaWorkspaceIDField = formTextField()
    quotaWorkspaceIDField.placeholderString = appText(
        "例如：wrk_abc123",
        "例如：wrk_abc123",
        "For example: wrk_abc123"
    )
    let quotaWorkspaceIDRow = labeledView(
        appText("工作区 ID", "工作區 ID", "Workspace ID"),
        quotaWorkspaceIDField
    )
    let quotaAuthCookieField = secureFormTextField()
    quotaAuthCookieField.placeholderString = appText(
        "opencode.ai 的 auth cookie",
        "opencode.ai 的 auth cookie",
        "opencode.ai auth cookie"
    )
    let quotaAuthCookieRow = labeledView(
        appText("Auth Cookie", "Auth Cookie", "Auth Cookie"),
        quotaAuthCookieField
    )
    let displayNameField = formTextField()
    displayNameField.placeholderString = appText(
        "例如：社区公益站",
        "例如：社群公益站",
        "For example: Community API"
    )
    let displayNameRow = labeledView(
        appText("站点名称", "站點名稱", "Site name"),
        displayNameField
    )
    let baseURLField = formTextField()
    baseURLField.placeholderString = "https://example.com/v1"
    let baseURLRow = labeledView(
        appText("API 地址", "API 位址", "API URL"),
        baseURLField
    )
    let baiduAuthBridgePopup = baiduAuthBridgePopUpButton()
    let baiduAuthBridgeRow = labeledView(
        appText("认证桥接", "認證橋接", "Auth bridge"),
        baiduAuthBridgePopup
    )

    let contentView = NSView(frame: NSRect(x: 0, y: 0, width: 650, height: 700))
    let panel = NSWindow(
        contentRect: contentView.frame,
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = appText("新增供应商", "新增供應商", "Add Provider")
    panel.contentView = contentView
    panel.isReleasedWhenClosed = false

    let titleLabel = NSTextField(
        labelWithString: appText("新增供应商", "新增供應商", "Add Provider")
    )
    titleLabel.font = .boldSystemFont(ofSize: 18)

    let detailLabel = NSTextField(wrappingLabelWithString: appText(
        "只需选择订阅并填写凭据。地址、协议、接口路径、额度规则和模型发现均由 Codex Mixin 自动配置。",
        "只需選擇訂閱並填寫憑證。地址、協議、端點路徑、額度規則和模型探索均由 Codex Mixin 自動設定。",
        "Choose a subscription and enter its credentials. Codex Mixin configures endpoints, protocols, quota rules, and model discovery automatically."
    ))
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    detailLabel.translatesAutoresizingMaskIntoConstraints = false
    detailLabel.widthAnchor.constraint(equalToConstant: 550).isActive = true

    let tokenButton = NSButton(
        title: appText("打开密钥页面", "開啟金鑰頁面", "Open API Key Page"),
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
        baiduAuthBridgeRow.isHidden = provider != "baidu-oneapi"
        tokenButton.isHidden = isCustom
    }
    providerPopup.target = providerTarget
    providerPopup.action = #selector(ModalActionTarget.run(_:))
    providerTarget.run(nil)

    let formStack = NSStackView(views: [
        labeledView(appText("供应商", "供應商", "Provider"), providerPopup),
        displayNameRow,
        baseURLRow,
        labeledView("API Key", apiKeyField),
        quotaUsernameRow,
        quotaWorkspaceIDRow,
        quotaAuthCookieRow,
        baiduAuthBridgeRow,
    ])
    formStack.orientation = .vertical
    formStack.spacing = 10

    let cancelButton = NSButton(
        title: appText("取消", "取消", "Cancel"),
        target: nil,
        action: nil
    )
    cancelButton.bezelStyle = .rounded
    let saveButton = NSButton(
        title: appText("添加", "新增", "Add"),
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
                title: appText("缺少自定义站点信息", "缺少自訂站點資訊", "Custom Site Information Required"),
                message: appText(
                    "请填写站点名称和 API 地址。地址可粘贴站点根地址、/v1 或完整推理接口，Codex Mixin 会自动识别。",
                    "請填寫站點名稱和 API 位址。可貼上站點根位址、/v1 或完整推理端點，Codex Mixin 會自動識別。",
                    "Enter the site name and API URL. You can paste the root URL, /v1 URL, or a full inference endpoint; Codex Mixin detects it automatically."
                )
            )
            return
        }
        let apiKey = apiKeyField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !apiKey.isEmpty else {
            showAlert(
                title: "缺少 API 密钥",
                message: appText(
                    "请填写供应商 API Key。",
                    "請填寫供應商 API Key。",
                    "Enter the provider API key."
                )
            )
            return
        }
        let username = quotaUsernameField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if preset == "baidu-oneapi", username.isEmpty {
            showAlert(
                title: "缺少额度用户名",
                message: appText(
                    "请填写 Baidu OneAPI 额度用户名。",
                    "請填寫 Baidu OneAPI 額度使用者名稱。",
                    "Enter the Baidu OneAPI quota username."
                )
            )
            return
        }
        let workspaceID = quotaWorkspaceIDField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let authCookie = quotaAuthCookieField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if requiresOpenCodeGoQuotaCredentials(preset), workspaceID.isEmpty || authCookie.isEmpty {
            showAlert(
                title: appText(
                    "缺少 OpenCode Go 额度凭据",
                    "缺少 OpenCode Go 額度憑證",
                    "OpenCode Go Quota Credentials Required"
                ),
                message: appText(
                    "OpenCode Go 需要同时填写工作区 ID 和 auth cookie 才能查询用量与余额。",
                    "OpenCode Go 需要同時填寫工作區 ID 和 auth cookie 才能查詢用量與餘額。",
                    "OpenCode Go requires both the workspace ID and auth cookie to show usage and balance."
                )
            )
            return
        }
        values = AddProviderFormValues(
            preset: preset,
            displayName: displayName,
            baseURL: baseURL,
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
        (appText("不使用（默认）", "不使用（預設）", "Disabled (default)"), "disabled"),
        (
            appText(
                "DUCC 核心（loopback）",
                "DUCC 核心（loopback）",
                "DUCC Core (loopback)"
            ),
            "ducc_loopback"
        ),
    ]
    for (title, value) in modes {
        popup.addItem(withTitle: title)
        popup.lastItem?.representedObject = value
    }
    popup.toolTip = appText(
        "DUCC 使用 Codex Mixin 管理的独立副本，在隔离 HOME 中下载、登录和运行，不修改系统配置或 hooks。",
        "DUCC 使用 Codex Mixin 管理的獨立副本，在隔離 HOME 中下載、登入和執行，不修改系統設定或 hooks。",
        "DUCC uses a Codex Mixin-managed copy and downloads, signs in, and runs inside an isolated HOME without changing system config or hooks."
    )
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
