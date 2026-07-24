import Cocoa

final class ProviderSettingsWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    typealias LoadHandler = () async throws -> ProviderListResponse
    typealias RunHandler = ([String]) async throws -> String
    typealias ApplyHandler = () async throws -> Void

    private let loadHandler: LoadHandler
    private let runHandler: RunHandler
    private let applyHandler: ApplyHandler

    private var response: ProviderListResponse?
    private var providers: [ProviderView] = []
    private var filteredModels: [ProviderModelListItem] = []
    private var selectedModelIDs: Set<String> = []
    private var isBusy = false

    private let providerTable = NSTableView()
    private let modelTable = NSTableView()
    private let searchField = NSSearchField()
    private let modelFilterPopup = NSPopUpButton()
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
    private let discoverButton = NSButton(title: "刷新模型", target: nil, action: nil)
    private let selectAllButton = NSButton(title: "全选", target: nil, action: nil)
    private let selectNoneButton = NSButton(title: "全不选", target: nil, action: nil)
    private let saveButton = NSButton(title: "保存更改", target: nil, action: nil)

    init(
        loadHandler: @escaping LoadHandler,
        runHandler: @escaping RunHandler,
        applyHandler: @escaping ApplyHandler
    ) {
        self.loadHandler = loadHandler
        self.runHandler = runHandler
        self.applyHandler = applyHandler
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1080, height: 720),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "供应商与模型"
        window.minSize = NSSize(width: 920, height: 620)
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
        tableView === providerTable ? providers.count : filteredModels.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let tableColumn else { return nil }
        if tableView === providerTable {
            guard providers.indices.contains(row) else { return nil }
            return providerCell(providers[row], identifier: tableColumn.identifier)
        }
        guard filteredModels.indices.contains(row) else { return nil }
        return modelCell(filteredModels[row], column: tableColumn)
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard let tableView = notification.object as? NSTableView, tableView === providerTable else {
            return
        }
        loadSelectedProvider()
    }

    func controlTextDidChange(_ obj: Notification) {
        guard let field = obj.object as? NSSearchField, field === searchField else { return }
        updateFilteredModels()
    }

    private var selectedProvider: ProviderView? {
        let row = providerTable.selectedRow
        return providers.indices.contains(row) ? providers[row] : nil
    }

    private func buildContent(in window: NSWindow) {
        guard let contentView = window.contentView else { return }

        let titleLabel = NSTextField(labelWithString: "供应商与模型")
        titleLabel.font = .boldSystemFont(ofSize: 20)
        let detailLabel = NSTextField(wrappingLabelWithString: "每个 Provider 独立保存凭据和模型 allowlist。预设站点自动管理连接细节；自定义站点只需填写名称、API 地址和密钥。")
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
        providerPane.widthAnchor.constraint(equalToConstant: 250).isActive = true

        configurePopups()
        configureFields()
        configureModelTable()
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
            managedConfigurationLabel,
        ])
        form.orientation = .vertical
        form.alignment = .leading
        form.spacing = 7

        let formDocument = NSView()
        form.translatesAutoresizingMaskIntoConstraints = false
        formDocument.addSubview(form)
        NSLayoutConstraint.activate([
            form.leadingAnchor.constraint(equalTo: formDocument.leadingAnchor),
            form.trailingAnchor.constraint(equalTo: formDocument.trailingAnchor),
            form.topAnchor.constraint(equalTo: formDocument.topAnchor),
            form.bottomAnchor.constraint(equalTo: formDocument.bottomAnchor),
            formDocument.widthAnchor.constraint(greaterThanOrEqualToConstant: 650),
        ])
        let formScroll = NSScrollView()
        formScroll.documentView = formDocument
        formScroll.hasVerticalScroller = true
        formScroll.autohidesScrollers = true
        formScroll.drawsBackground = false
        formScroll.translatesAutoresizingMaskIntoConstraints = false
        formScroll.heightAnchor.constraint(equalToConstant: 345).isActive = true

        searchField.placeholderString = "搜索模型"
        searchField.delegate = self
        searchField.translatesAutoresizingMaskIntoConstraints = false
        searchField.widthAnchor.constraint(greaterThanOrEqualToConstant: 220).isActive = true
        configureButton(selectAllButton, action: #selector(selectAllModels))
        configureButton(selectNoneButton, action: #selector(selectNoModels))
        let modelControls = NSStackView(views: [
            searchField,
            modelFilterPopup,
            NSView(),
            selectAllButton,
            selectNoneButton,
        ])
        modelControls.orientation = .horizontal
        modelControls.alignment = .centerY
        modelControls.spacing = 8

        let modelScroll = NSScrollView()
        modelScroll.documentView = modelTable
        modelScroll.hasVerticalScroller = true
        modelScroll.autohidesScrollers = true
        modelScroll.borderType = .bezelBorder
        modelScroll.translatesAutoresizingMaskIntoConstraints = false

        configureButton(enableButton, action: #selector(toggleProvider))
        configureButton(testButton, action: #selector(testProvider))
        configureButton(discoverButton, action: #selector(discoverModels))
        configureButton(saveButton, action: #selector(saveProvider))
        saveButton.keyEquivalent = "\r"
        let actionRow = NSStackView(views: [enableButton, testButton, discoverButton, NSView(), saveButton])
        actionRow.orientation = .horizontal
        actionRow.alignment = .centerY
        actionRow.spacing = 9

        statusLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        statusLabel.textColor = .secondaryLabelColor
        statusLabel.lineBreakMode = .byTruncatingMiddle

        let detailsPane = NSStackView(views: [formScroll, modelControls, modelScroll, actionRow, statusLabel])
        detailsPane.orientation = .vertical
        detailsPane.spacing = 10
        detailsPane.translatesAutoresizingMaskIntoConstraints = false

        emptyLabel.textColor = .secondaryLabelColor
        emptyLabel.alignment = .center
        emptyLabel.translatesAutoresizingMaskIntoConstraints = false

        let body = NSStackView(views: [providerPane, detailsPane])
        body.orientation = .horizontal
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
            body.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -20),
            detailsPane.widthAnchor.constraint(greaterThanOrEqualToConstant: 620),

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

    private func configureModelTable() {
        let selected = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("selected"))
        selected.title = "加入 Codex"
        selected.width = 90
        selected.minWidth = 80
        let model = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("model"))
        model.title = "上游模型"
        model.width = 430
        model.minWidth = 240
        model.resizingMask = [.autoresizingMask, .userResizingMask]
        let context = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("context"))
        context.title = "Context"
        context.width = 100
        modelTable.addTableColumn(selected)
        modelTable.addTableColumn(model)
        modelTable.addTableColumn(context)
        modelTable.delegate = self
        modelTable.dataSource = self
        modelTable.rowHeight = 28
        modelTable.usesAlternatingRowBackgroundColors = true
        modelTable.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
    }

    private func configurePopups() {
        for item in [
            ("全部模型", "all"),
            ("已选", "selected"),
            ("新增", "new"),
            ("不可用", "unavailable"),
        ] {
            modelFilterPopup.addItem(withTitle: item.0)
            modelFilterPopup.lastItem?.representedObject = item.1
        }
        modelFilterPopup.target = self
        modelFilterPopup.action = #selector(changeModelFilter)
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

    private func modelCell(_ model: ProviderModelListItem, column: NSTableColumn) -> NSView {
        let identifier = column.identifier
        if identifier.rawValue == "selected" {
            let button = NSButton(checkboxWithTitle: "", target: self, action: #selector(toggleModel(_:)))
            button.state = selectedModelIDs.contains(model.id) ? .on : .off
            button.identifier = NSUserInterfaceItemIdentifier(model.id)
            let cell = NSTableCellView()
            cell.addSubview(button)
            button.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                button.centerXAnchor.constraint(equalTo: cell.centerXAnchor),
                button.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
            return cell
        }
        let cell: NSTableCellView
        if let reused = modelTable.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView {
            cell = reused
        } else {
            cell = NSTableCellView()
            cell.identifier = identifier
            let field = NSTextField(labelWithString: "")
            field.translatesAutoresizingMaskIntoConstraints = false
            field.lineBreakMode = .byTruncatingMiddle
            cell.textField = field
            cell.addSubview(field)
            NSLayoutConstraint.activate([
                field.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
                field.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
                field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }
        if identifier.rawValue == "model" {
            let name = model.displayName.flatMap { $0 == model.id ? nil : $0 }
            var labels: [String] = []
            if model.isNew {
                labels.append("新增")
            }
            if !model.isAvailable {
                labels.append("不可用")
            }
            let status = labels.isEmpty ? "" : " [\(labels.joined(separator: " · "))]"
            cell.textField?.stringValue = (name.map { "\(model.id) · \($0)" } ?? model.id) + status
            cell.textField?.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
            cell.textField?.textColor = model.isAvailable ? .labelColor : .secondaryLabelColor
            cell.toolTip = model.description
        } else {
            cell.textField?.stringValue = model.contextWindow.map(formatContextWindow) ?? "-"
            cell.textField?.font = .systemFont(ofSize: 11)
        }
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
                response = loaded
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
        selectedModelIDs = Set(provider.selectedModels)
        searchField.stringValue = ""
        selectPopupValue(modelFilterPopup, "all")
        updateFilteredModels()
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
        selectedModelIDs = []
        filteredModels = []
        modelTable.reloadData()
        statusLabel.stringValue = providers.isEmpty ? "等待新增 Provider" : "请选择 Provider"
        statusLabel.toolTip = nil
    }

    private func updateFilteredModels() {
        guard let provider = selectedProvider else {
            filteredModels = []
            modelTable.reloadData()
            return
        }
        let query = searchField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let filter = selectedPopupValue(modelFilterPopup, fallback: "all")
        filteredModels = provider.modelItems.filter { model in
            let matchesQuery = query.isEmpty
                || model.id.localizedCaseInsensitiveContains(query)
                || model.displayName?.localizedCaseInsensitiveContains(query) == true
            let matchesFilter = switch filter {
            case "selected":
                selectedModelIDs.contains(model.id)
            case "new":
                model.isNew
            case "unavailable":
                !model.isAvailable
            default:
                true
            }
            return matchesQuery && matchesFilter
        }
        modelTable.reloadData()
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
            searchField,
            modelFilterPopup,
            modelTable,
            enableButton,
            testButton,
            discoverButton,
            selectAllButton,
            selectNoneButton,
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
        guard !isBusy, let values = runAddProviderPanel() else { return }
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

    @objc private func discoverModels() {
        guard let provider = selectedProvider, !isBusy else { return }
        performMutation(
            ["providers", "discover", provider.id],
            status: "正在刷新 \(provider.id) 的模型…",
            selecting: provider.id
        )
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

    @objc private func changeModelFilter() {
        updateFilteredModels()
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
        let selectedModels = provider.modelItems
            .map(\.id)
            .filter { selectedModelIDs.contains($0) }
        var select = ["providers", "select", provider.id]
        for model in selectedModels {
            select.append(contentsOf: ["--model", model])
        }
        performMutation(
            update,
            then: select,
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

    @objc private func selectAllModels() {
        selectedModelIDs.formUnion(filteredModels.map(\.id))
        updateFilteredModels()
    }

    @objc private func selectNoModels() {
        selectedModelIDs.subtract(filteredModels.map(\.id))
        updateFilteredModels()
    }

    @objc private func toggleModel(_ sender: NSButton) {
        guard let modelID = sender.identifier?.rawValue else { return }
        if sender.state == .on {
            selectedModelIDs.insert(modelID)
        } else {
            selectedModelIDs.remove(modelID)
        }
        updateFilteredModels()
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
    label.widthAnchor.constraint(equalToConstant: 96).isActive = true
    field.translatesAutoresizingMaskIntoConstraints = false
    field.widthAnchor.constraint(equalToConstant: 520).isActive = true
    let row = NSStackView(views: [label, field])
    row.orientation = .horizontal
    row.alignment = .centerY
    row.spacing = 9
    return row
}

func formatContextWindow(_ value: UInt64) -> String {
    if value >= 1_000_000 {
        return String(format: "%.1fM", Double(value) / 1_000_000)
    }
    if value >= 1_000 {
        return String(format: "%.0fK", Double(value) / 1_000)
    }
    return "\(value)"
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
