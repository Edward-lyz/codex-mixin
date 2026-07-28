import Cocoa

final class ProviderSettingsWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate {
    typealias LoadHandler = () async throws -> ProviderListResponse
    typealias RunHandler = ([String]) async throws -> String
    typealias ApplyHandler = () async throws -> Void

    private let loadHandler: LoadHandler
    private let runHandler: RunHandler
    private let applyHandler: ApplyHandler

    private var providers: [ProviderView] = []
    private var isBusy = false

    private let providerTable = NSTableView()
    private let statusLabel = NSTextField(labelWithString: "正在读取供应商…")
    private let emptyLabel = NSTextField(labelWithString: "还没有供应商，点击“新增”开始配置。")

    private let idField = copyableTextField("")
    private let displayNameField = formTextField()
    private let baseURLField = formTextField()
    private let apiKeyField = secureFormTextField()
    private let clearKeyButton = NSButton(title: "清除密钥", target: nil, action: nil)
    private let quotaUsernameField = formTextField()
    private var customDisplayNameRow: NSView?
    private var customBaseURLRow: NSView?
    private var quotaUsernameRow: NSView?

    private let addButton = NSButton(title: "新增", target: nil, action: nil)
    private let removeButton = NSButton(title: "删除", target: nil, action: nil)
    private let enableButton = NSButton(title: "停用", target: nil, action: nil)
    private let testButton = NSButton(title: "测试连接", target: nil, action: nil)
    private let saveButton = NSButton(title: "保存更改", target: nil, action: nil)

    init(
        loadHandler: @escaping LoadHandler,
        runHandler: @escaping RunHandler,
        applyHandler: @escaping ApplyHandler
    ) {
        self.loadHandler = loadHandler
        self.runHandler = runHandler
        self.applyHandler = applyHandler
        let visibleFrame = NSScreen.main?.visibleFrame
            ?? NSRect(x: 0, y: 0, width: 1_280, height: 800)
        let contentSize = providerSettingsContentSize(for: visibleFrame)
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: contentSize),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "供应商设置"
        window.minSize = NSSize(width: 720, height: 400)
        window.isReleasedWhenClosed = false
        window.center()
        super.init(window: window)
        window.delegate = self
        buildContent(in: window)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        reloadProviders()
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        providers.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let tableColumn, providers.indices.contains(row) else { return nil }
        return providerCell(providers[row], identifier: tableColumn.identifier)
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard let tableView = notification.object as? NSTableView, tableView === providerTable else {
            return
        }
        loadSelectedProvider()
    }

    private var selectedProvider: ProviderView? {
        let row = providerTable.selectedRow
        return providers.indices.contains(row) ? providers[row] : nil
    }

    private func buildContent(in window: NSWindow) {
        guard let contentView = window.contentView else { return }

        let titleLabel = NSTextField(labelWithString: "供应商设置")
        titleLabel.font = .boldSystemFont(ofSize: 20)
        let detailLabel = NSTextField(wrappingLabelWithString: "这里只配置供应商地址、密钥与启停。模型勾选、刷新模型、性能对比与测速请使用独立的“模型选择与测速…”入口。")
        detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        detailLabel.textColor = .secondaryLabelColor

        let header = NSStackView(views: [titleLabel, detailLabel])
        header.orientation = .vertical
        header.alignment = .leading
        header.spacing = 5
        header.translatesAutoresizingMaskIntoConstraints = false

        configureProviderTable()
        let providerScroll = NSScrollView()
        providerScroll.documentView = providerTable
        providerScroll.hasVerticalScroller = true
        providerScroll.autohidesScrollers = true
        providerScroll.borderType = .bezelBorder
        providerScroll.translatesAutoresizingMaskIntoConstraints = false
        providerScroll.heightAnchor.constraint(equalToConstant: 238).isActive = true

        configureButton(addButton, action: #selector(addProvider))
        configureButton(removeButton, action: #selector(removeProvider))
        let providerButtons = NSStackView(views: [addButton, removeButton])
        providerButtons.orientation = .horizontal
        providerButtons.distribution = .fillEqually
        providerButtons.spacing = 8

        let providerPane = NSStackView(views: [providerScroll, providerButtons])
        providerPane.orientation = .vertical
        providerPane.spacing = 10
        providerPane.translatesAutoresizingMaskIntoConstraints = false
        providerPane.widthAnchor.constraint(equalToConstant: 220).isActive = true

        configureFields()
        configureButton(clearKeyButton, action: #selector(clearProviderKey))
        let apiKeyControls = NSStackView(views: [apiKeyField, clearKeyButton])
        apiKeyControls.orientation = .horizontal
        apiKeyControls.alignment = .centerY
        apiKeyControls.spacing = 8

        let quotaUsernameRow = compactLabeledView("额度用户名", quotaUsernameField)
        self.quotaUsernameRow = quotaUsernameRow
        let customDisplayNameRow = compactLabeledView("站点名称", displayNameField)
        self.customDisplayNameRow = customDisplayNameRow
        let customBaseURLRow = compactLabeledView("API 地址", baseURLField)
        self.customBaseURLRow = customBaseURLRow
        let managedConfigurationLabel = NSTextField(wrappingLabelWithString: appText(
            "协议和接口路径会自动识别，不需要手动选择。",
            "協議和端點路徑會自動識別，不需要手動選擇。",
            "Protocols and endpoint paths are detected automatically."
        ))
        managedConfigurationLabel.textColor = .secondaryLabelColor
        managedConfigurationLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        let form = NSStackView(views: [
            compactLabeledView("Provider ID", idField),
            customDisplayNameRow,
            customBaseURLRow,
            compactLabeledView("API 密钥", apiKeyControls),
            quotaUsernameRow,
            compactLabeledView("", managedConfigurationLabel),
        ])
        form.orientation = .vertical
        form.alignment = .leading
        form.spacing = 9
        form.translatesAutoresizingMaskIntoConstraints = false

        let sectionTitle = NSTextField(labelWithString: "连接配置")
        sectionTitle.font = .systemFont(ofSize: 14, weight: .semibold)

        configureButton(enableButton, action: #selector(toggleProvider))
        configureButton(testButton, action: #selector(testProvider))
        configureButton(saveButton, action: #selector(saveProvider))
        saveButton.keyEquivalent = "\r"
        let actionRow = NSStackView(views: [
            enableButton,
            testButton,
            NSView(),
            saveButton,
        ])
        actionRow.orientation = .horizontal
        actionRow.alignment = .centerY
        actionRow.spacing = 9

        statusLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        statusLabel.textColor = .secondaryLabelColor
        statusLabel.lineBreakMode = .byTruncatingMiddle

        let detailsPane = NSStackView(views: [sectionTitle, form, actionRow, statusLabel])
        detailsPane.orientation = .vertical
        detailsPane.alignment = .leading
        detailsPane.spacing = 12
        detailsPane.translatesAutoresizingMaskIntoConstraints = false

        emptyLabel.textColor = .secondaryLabelColor
        emptyLabel.alignment = .center
        emptyLabel.translatesAutoresizingMaskIntoConstraints = false

        let body = NSStackView(views: [providerPane, detailsPane])
        body.orientation = .horizontal
        body.alignment = .top
        body.spacing = 16
        body.translatesAutoresizingMaskIntoConstraints = false

        contentView.addSubview(header)
        contentView.addSubview(body)
        contentView.addSubview(emptyLabel)
        NSLayoutConstraint.activate([
            header.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 24),
            header.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -24),
            header.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 20),

            body.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            body.trailingAnchor.constraint(equalTo: header.trailingAnchor),
            body.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 16),
            body.bottomAnchor.constraint(lessThanOrEqualTo: contentView.bottomAnchor, constant: -20),
            detailsPane.widthAnchor.constraint(greaterThanOrEqualToConstant: 476),
            form.widthAnchor.constraint(equalTo: detailsPane.widthAnchor),
            actionRow.widthAnchor.constraint(equalTo: detailsPane.widthAnchor),
            statusLabel.widthAnchor.constraint(equalTo: detailsPane.widthAnchor),

            emptyLabel.centerXAnchor.constraint(equalTo: detailsPane.centerXAnchor),
            emptyLabel.centerYAnchor.constraint(equalTo: detailsPane.centerYAnchor),
        ])
        setDetailControlsEnabled(false)
    }

    private func configureProviderTable() {
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("provider"))
        column.title = "Provider"
        column.width = 235
        providerTable.addTableColumn(column)
        providerTable.headerView = nil
        providerTable.delegate = self
        providerTable.dataSource = self
        providerTable.rowHeight = 42
        providerTable.allowsMultipleSelection = false
        providerTable.usesAlternatingRowBackgroundColors = true
    }

    private func configureFields() {
        idField.font = .monospacedSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
        apiKeyField.placeholderString = "留空保留已保存密钥"
        quotaUsernameField.placeholderString = "Baidu OneAPI 额度接口必填"
    }

    private func configureButton(_ button: NSButton, action: Selector) {
        button.bezelStyle = .rounded
        button.target = self
        button.action = action
    }

    private func providerCell(_ provider: ProviderView, identifier: NSUserInterfaceItemIdentifier) -> NSView {
        let cell: NSTableCellView
        if let reused = providerTable.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView {
            cell = reused
        } else {
            cell = NSTableCellView()
            cell.identifier = identifier
            let title = NSTextField(labelWithString: "")
            title.font = .systemFont(ofSize: 13, weight: .medium)
            title.translatesAutoresizingMaskIntoConstraints = false
            let detail = NSTextField(labelWithString: "")
            detail.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
            detail.textColor = .secondaryLabelColor
            detail.translatesAutoresizingMaskIntoConstraints = false
            let stack = NSStackView(views: [title, detail])
            stack.orientation = .vertical
            stack.alignment = .leading
            stack.spacing = 2
            stack.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(stack)
            cell.textField = title
            cell.identifier = identifier
            detail.identifier = NSUserInterfaceItemIdentifier("detail")
            NSLayoutConstraint.activate([
                stack.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 7),
                stack.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -7),
                stack.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                title.widthAnchor.constraint(equalTo: stack.widthAnchor),
                detail.widthAnchor.constraint(equalTo: stack.widthAnchor),
            ])
        }
        cell.textField?.stringValue = provider.displayName
        let detail = cell.subviews
            .compactMap { $0 as? NSStackView }
            .flatMap(\.arrangedSubviews)
            .first { $0.identifier?.rawValue == "detail" } as? NSTextField
        detail?.stringValue = "\(provider.id) · \(readinessLabel(provider.readiness)) · 已选 \(provider.selectedModels.count) / 可用 \(provider.cachedModels.count)"
        return cell
    }

    private func reloadProviders(selecting providerID: String? = nil) {
        guard !isBusy else { return }
        setBusy(true, status: "正在读取供应商…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer { setBusy(false, status: selectedProviderStatus()) }
            do {
                let previousID = providerID ?? selectedProvider?.id
                let loaded = try await loadHandler()
                providers = loaded.providers
                providerTable.reloadData()
                emptyLabel.isHidden = !providers.isEmpty
                if let previousID, let row = providers.firstIndex(where: { $0.id == previousID }) {
                    providerTable.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
                } else if !providers.isEmpty {
                    providerTable.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
                } else {
                    providerTable.deselectAll(nil)
                    loadSelectedProvider()
                }
            } catch {
                statusLabel.stringValue = "读取失败"
                showAlert(title: "读取供应商失败", message: String(describing: error))
            }
        }
    }

    private func loadSelectedProvider() {
        guard let provider = selectedProvider else {
            clearDetails()
            setDetailControlsEnabled(false)
            emptyLabel.isHidden = !providers.isEmpty
            return
        }
        emptyLabel.isHidden = true
        setDetailControlsEnabled(!isBusy)
        idField.stringValue = provider.id
        displayNameField.stringValue = provider.displayName
        baseURLField.stringValue = provider.baseURL
        apiKeyField.stringValue = ""
        apiKeyField.placeholderString = provider.apiKeyConfigured
            ? "已配置；留空保留"
            : "尚未配置；启用前必须填写"
        quotaUsernameField.stringValue = provider.quotaUsername ?? ""
        let isCustom = provider.presetID == "custom"
        customDisplayNameRow?.isHidden = !isCustom
        customBaseURLRow?.isHidden = !isCustom
        quotaUsernameRow?.isHidden = provider.presetID != "baidu-oneapi"
        enableButton.title = provider.enabled ? "停用" : "启用"
        statusLabel.stringValue = selectedProviderStatus()
        statusLabel.toolTip = provider.lastModelRefreshError
    }

    private func clearDetails() {
        for field in [
            idField,
            displayNameField,
            baseURLField,
            apiKeyField,
            quotaUsernameField,
        ] {
            field.stringValue = ""
        }
        statusLabel.stringValue = providers.isEmpty ? "等待新增 Provider" : "请选择 Provider"
        statusLabel.toolTip = nil
    }

    private func setBusy(_ busy: Bool, status: String) {
        isBusy = busy
        statusLabel.stringValue = status
        setDetailControlsEnabled(!busy && selectedProvider != nil)
        addButton.isEnabled = !busy
        removeButton.isEnabled = !busy && selectedProvider != nil
    }

    private func setDetailControlsEnabled(_ enabled: Bool) {
        let controls: [NSControl] = [
            apiKeyField,
            displayNameField,
            baseURLField,
            quotaUsernameField,
            enableButton,
            testButton,
            saveButton,
        ]
        for control in controls {
            control.isEnabled = enabled
        }
        clearKeyButton.isEnabled = enabled && selectedProvider?.apiKeyConfigured == true
    }

    private func selectedProviderStatus() -> String {
        guard let provider = selectedProvider else {
            return providers.isEmpty ? "等待新增 Provider" : "请选择 Provider"
        }
        let refresh: String
        if let milliseconds = provider.modelsRefreshedAtMilliseconds {
            refresh = "模型缓存更新于 \(formatProviderTimestamp(milliseconds))"
        } else {
            refresh = "尚未在线刷新模型"
        }
        var details = [
            "\(provider.routableModelCount) 个模型可路由",
            "\(provider.newModels.count) 个新增",
            "\(provider.unavailableSelectedModels.count) 个不可用",
            refresh,
        ]
        if provider.lastModelRefreshError != nil {
            details.append("上次刷新失败")
        }
        return details.joined(separator: " · ")
    }

    @objc private func addProvider() {
        guard !isBusy, let window else { return }
        runAddProviderSheet(attachedTo: window) { [weak self] values in
            guard let self, let values else { return }
            submitNewProvider(values)
        }
    }

    private func submitNewProvider(_ values: AddProviderFormValues) {
        let id = nextProviderID(for: values.preset)
        let key = values.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else {
            showAlert(title: "缺少 API 密钥", message: "新增 Provider 必须填写 API 密钥。")
            return
        }
        var arguments = ["providers", "add", "--preset", values.preset, "--id", id, "--key", key]
        if values.preset == "custom" {
            appendProviderArgument(&arguments, "--display-name", values.displayName)
            appendProviderArgument(&arguments, "--base-url", values.baseURL)
        }
        appendProviderArgument(&arguments, "--quota-username", values.quotaUsername)
        performMutation(
            arguments,
            then: ["providers", "discover", id],
            status: "正在新增并发现模型 \(id)…",
            selecting: id
        )
    }

    @objc private func removeProvider() {
        guard let provider = selectedProvider, !isBusy else { return }
        guard confirm(
            title: "删除 \(provider.displayName)？",
            message: "将删除 Provider \(provider.id) 的地址、密钥和模型选择。被 Fusion 引用时 CLI 会拒绝删除。"
        ) else { return }
        performMutation(
            ["providers", "remove", provider.id],
            status: "正在删除 \(provider.id)…",
            selecting: nil
        )
    }

    @objc private func toggleProvider() {
        guard let provider = selectedProvider, !isBusy else { return }
        let action = provider.enabled ? "disable" : "enable"
        performMutation(
            ["providers", action, provider.id],
            status: "正在\(provider.enabled ? "停用" : "启用") \(provider.id)…",
            selecting: provider.id
        )
    }

    @objc private func testProvider() {
        guard let provider = selectedProvider, !isBusy else { return }
        setBusy(true, status: "正在测试 \(provider.id)…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer { setBusy(false, status: selectedProviderStatus()) }
            do {
                let output = try await runHandler(["providers", "test", provider.id, "--json"])
                let result = try decodeProviderTest(output)
                let mode = result.mode == "configuration" ? "静态模型配置检查" : "模型接口检查"
                showAlert(
                    title: "连接测试通过",
                    message: "\(provider.displayName)：\(mode)，发现 \(result.modelCount) 个模型；没有发起付费推理。"
                )
            } catch {
                showAlert(title: "连接测试失败", message: String(describing: error))
            }
        }
    }

    @objc private func clearProviderKey() {
        guard let provider = selectedProvider, !isBusy, provider.apiKeyConfigured else { return }
        guard !provider.enabled else {
            showAlert(
                title: "请先停用 Provider",
                message: "为避免让启用中的 Provider 进入无密钥状态，请先停用 \(provider.displayName)，再清除密钥。"
            )
            return
        }
        guard confirm(
            title: "清除 \(provider.displayName) 的密钥？",
            message: "此操作会永久移除已保存的 API 密钥。之后必须重新填写密钥才能启用该 Provider。"
        ) else { return }
        performMutation(
            ["providers", "update", provider.id, "--clear-key"],
            status: "正在清除 \(provider.id) 的密钥…",
            selecting: provider.id
        )
    }

    @objc private func saveProvider() {
        guard let provider = selectedProvider, !isBusy else { return }
        var update = ["providers", "update", provider.id]
        appendProviderArgument(&update, "--key", apiKeyField.stringValue)
        if provider.presetID == "custom" {
            let displayName = displayNameField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            let baseURL = baseURLField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !displayName.isEmpty, !baseURL.isEmpty else {
                showAlert(
                    title: appText("缺少自定义站点信息", "缺少自訂站點資訊", "Custom Site Information Required"),
                    message: appText(
                        "站点名称和 API 地址不能为空。",
                        "站點名稱和 API 位址不能為空。",
                        "Site name and API URL cannot be empty."
                    )
                )
                return
            }
            appendProviderArgument(&update, "--display-name", displayName)
            appendProviderArgument(&update, "--base-url", baseURL)
        }
        let quotaUsername = quotaUsernameField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if provider.presetID == "baidu-oneapi", quotaUsername.isEmpty {
            showAlert(
                title: "缺少额度用户名",
                message: "Baidu OneAPI 查询额度必须填写用户名。"
            )
            return
        }
        if provider.presetID == "baidu-oneapi" {
            appendProviderArgument(&update, "--quota-username", quotaUsername)
        }
        performMutation(
            update,
            status: "正在保存 \(provider.id)…",
            selecting: provider.id
        )
    }

    private func nextProviderID(for preset: String) -> String {
        let existing = Set(providers.map(\.id))
        if !existing.contains(preset) {
            return preset
        }
        var suffix = 2
        while existing.contains("\(preset)-\(suffix)") {
            suffix += 1
        }
        return "\(preset)-\(suffix)"
    }

    private func performMutation(
        _ arguments: [String],
        then secondArguments: [String]? = nil,
        status: String,
        selecting providerID: String?
    ) {
        guard !isBusy else { return }
        setBusy(true, status: status)
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await runHandler(arguments)
                if let secondArguments {
                    _ = try await runHandler(secondArguments)
                }
                try await applyHandler()
                setBusy(false, status: "配置已保存")
                reloadProviders(selecting: providerID)
                showAlert(
                    title: appText(
                        "供应商配置已更新",
                        "供應商設定已更新",
                        "Provider Configuration Updated"
                    ),
                    message: appText(
                        "Codex 模型目录已重新生成。请完全退出并重新打开 Codex App；Codex CLI 请开启新会话。",
                        "Codex 模型目錄已重新產生。請完全結束並重新開啟 Codex App；Codex CLI 請開啟新工作階段。",
                        "The Codex model catalog has been regenerated. Fully quit and reopen the Codex App, and start a new Codex CLI session."
                    )
                )
            } catch {
                setBusy(false, status: "操作失败")
                showAlert(title: "供应商操作失败", message: String(describing: error))
                reloadProviders(selecting: providerID)
            }
        }
    }
}

func compactLabeledView(_ title: String, _ field: NSView) -> NSView {
    let label = NSTextField(labelWithString: title)
    label.alignment = .right
    label.textColor = .secondaryLabelColor
    label.translatesAutoresizingMaskIntoConstraints = false
    label.widthAnchor.constraint(equalToConstant: 78).isActive = true
    field.translatesAutoresizingMaskIntoConstraints = false
    field.widthAnchor.constraint(equalToConstant: 390).isActive = true
    let row = NSStackView(views: [label, field])
    row.orientation = .horizontal
    row.alignment = .centerY
    row.spacing = 8
    return row
}

func formatProviderTimestamp(_ milliseconds: UInt64) -> String {
    let date = Date(timeIntervalSince1970: TimeInterval(milliseconds) / 1_000)
    let formatter = DateFormatter()
    formatter.dateStyle = .short
    formatter.timeStyle = .short
    return formatter.string(from: date)
}

func readinessLabel(_ readiness: String) -> String {
    switch readiness {
    case "healthy":
        return "正常"
    case "degraded":
        return "降级"
    case "disabled":
        return "停用"
    default:
        return readiness
    }
}
