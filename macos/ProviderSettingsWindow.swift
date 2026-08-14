import Cocoa

private final class FlippedProviderSettingsView: NSView {
    override var isFlipped: Bool { true }
}

final class ProviderSettingsWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate {
    typealias LoadHandler = () async throws -> ProviderListResponse
    typealias RunHandler = ([String]) async throws -> String
    typealias ApplyHandler = (_ progress: OperationProgress?) async throws -> Void
    typealias BaiduBridgeSetupHandler = (BaiduAuthBridgeMode) async throws -> URL
    typealias CompletionHandler = (_ title: String, _ message: String) -> Void

    private let loadHandler: LoadHandler
    private let runHandler: RunHandler
    private let applyHandler: ApplyHandler
    private let baiduBridgeSetupHandler: BaiduBridgeSetupHandler?
    private let completionHandler: CompletionHandler?

    private var providers: [ProviderView] = []
    private var codexInstallMode: ManagedCodexInstallMode?
    private var isBusy = false
    private var remindedBaiduBridgeProviderIDs = Set<String>()
    private(set) var baiduBridgeReminderAlert: NSAlert?

    private let providerScrollView = NSScrollView()
    private let providerTable = NSTableView()
    private let statusLabel = NSTextField(labelWithString: "正在读取供应商…")
    private let emptyLabel = NSTextField(labelWithString: "还没有供应商，点击“新增”开始配置。")
    private let emptyIconView = NSImageView()

    private let bannerView = NSView()
    private let bannerLabel = NSTextField(labelWithString: "")
    private var bannerHeightConstraint: NSLayoutConstraint?
    private var bannerHideWorkItem: DispatchWorkItem?

    private let idField = copyableTextField("")
    private let displayNameField = formTextField()
    private let baseURLField = formTextField()
    private let websiteURLField = formTextField()
    private let imageGenerationPathField = formTextField()
    private let apiKeyField = secureFormTextField()
    private let clearKeyButton = NSButton(title: "清除密钥", target: nil, action: nil)
    private let quotaUsernameField = formTextField()
    private let quotaWorkspaceIDField = formTextField()
    private let quotaAuthCookieField = secureFormTextField()
    private let clearQuotaCredentialsButton = NSButton(
        title: AppLocalization.string("providerSettings.clearQuotaCredentials"),
        target: nil,
        action: nil
    )
    private let auxiliaryModelUpstreamButton = NSButton(
        checkboxWithTitle: AppLocalization.string("providerSettings.useForVoiceAutoReviewAndOther"),
        target: nil,
        action: nil
    )
    private let baiduAuthBridgePopup = baiduAuthBridgePopUpButton()
    private let baiduCodeReportButton = NSButton(
        checkboxWithTitle: "上报 AI 代码使用数据",
        target: nil,
        action: nil
    )
    private var customDisplayNameRow: NSView?
    private var customBaseURLRow: NSView?
    private var customWebsiteURLRow: NSView?
    private var quotaUsernameRow: NSView?
    private var quotaWorkspaceIDRow: NSView?
    private var quotaAuthCookieRow: NSView?
    private var baiduAuthBridgeRow: NSView?
    private var baiduCodeReportRow: NSView?

    private let addButton = NSButton(title: "新增", target: nil, action: nil)
    private let removeButton = NSButton(title: "删除", target: nil, action: nil)
    private let enableButton = NSButton(title: "停用", target: nil, action: nil)
    private let testButton = NSButton(title: "测试连接", target: nil, action: nil)
    private let saveButton = NSButton(title: "保存更改", target: nil, action: nil)

    init(
        loadHandler: @escaping LoadHandler,
        runHandler: @escaping RunHandler,
        applyHandler: @escaping ApplyHandler,
        baiduBridgeSetupHandler: BaiduBridgeSetupHandler? = nil,
        completionHandler: CompletionHandler? = nil
    ) {
        self.loadHandler = loadHandler
        self.runHandler = runHandler
        self.applyHandler = applyHandler
        self.baiduBridgeSetupHandler = baiduBridgeSetupHandler
        self.completionHandler = completionHandler
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
        window.minSize = NSSize(width: 820, height: 580)
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

        bannerView.wantsLayer = true
        bannerView.layer?.cornerRadius = 6
        bannerView.alphaValue = 0
        bannerView.isHidden = true
        bannerView.translatesAutoresizingMaskIntoConstraints = false
        bannerLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .medium)
        bannerLabel.lineBreakMode = .byTruncatingTail
        bannerLabel.translatesAutoresizingMaskIntoConstraints = false
        bannerView.addSubview(bannerLabel)
        bannerHeightConstraint = bannerView.heightAnchor.constraint(equalToConstant: 0)

        configureProviderTable()
        providerScrollView.documentView = providerTable
        providerScrollView.hasVerticalScroller = true
        providerScrollView.autohidesScrollers = true
        providerScrollView.borderType = .bezelBorder
        providerScrollView.translatesAutoresizingMaskIntoConstraints = false
        providerTable.frame = NSRect(x: 0, y: 0, width: 230, height: 1)
        providerTable.autoresizingMask = [.width]

        configureButton(addButton, action: #selector(addProvider))
        configureButton(removeButton, action: #selector(removeProvider))
        let providerButtons = NSStackView(views: [addButton, removeButton])
        providerButtons.orientation = .horizontal
        providerButtons.distribution = .fillEqually
        providerButtons.spacing = 8

        let providerPane = NSStackView(views: [providerScrollView, providerButtons])
        providerPane.orientation = .vertical
        providerPane.spacing = 10
        providerPane.translatesAutoresizingMaskIntoConstraints = false
        providerPane.widthAnchor.constraint(equalToConstant: 230).isActive = true

        configureFields()
        configureButton(clearKeyButton, action: #selector(clearProviderKey))
        configureButton(clearQuotaCredentialsButton, action: #selector(clearQuotaCredentials))
        let apiKeyControls = NSStackView(views: [apiKeyField, clearKeyButton])
        apiKeyControls.orientation = .horizontal
        apiKeyControls.alignment = .centerY
        apiKeyControls.spacing = 8

        let quotaUsernameRow = compactLabeledView("额度用户名", quotaUsernameField)
        self.quotaUsernameRow = quotaUsernameRow
        let quotaWorkspaceIDRow = compactLabeledView(
            AppLocalization.string("providerSettings.workspaceID"),
            quotaWorkspaceIDField
        )
        self.quotaWorkspaceIDRow = quotaWorkspaceIDRow
        quotaAuthCookieField.translatesAutoresizingMaskIntoConstraints = false
        quotaAuthCookieField.widthAnchor.constraint(equalToConstant: 300).isActive = true
        clearQuotaCredentialsButton.bezelStyle = .rounded
        clearQuotaCredentialsButton.controlSize = .small
        let quotaAuthCookieControls = NSStackView(views: [
            quotaAuthCookieField,
            clearQuotaCredentialsButton,
        ])
        quotaAuthCookieControls.orientation = .horizontal
        quotaAuthCookieControls.alignment = .centerY
        quotaAuthCookieControls.spacing = 8
        quotaAuthCookieControls.translatesAutoresizingMaskIntoConstraints = false
        let quotaAuthCookieRow = compactLabeledView(
            AppLocalization.string("providerSettings.authCookie"),
            quotaAuthCookieControls
        )
        self.quotaAuthCookieRow = quotaAuthCookieRow
        let customDisplayNameRow = compactLabeledView("站点名称", displayNameField)
        self.customDisplayNameRow = customDisplayNameRow
        let customBaseURLRow = compactLabeledView("API 地址", baseURLField)
        self.customBaseURLRow = customBaseURLRow
        let customWebsiteURLRow = compactLabeledView("官网地址", websiteURLField)
        self.customWebsiteURLRow = customWebsiteURLRow
        imageGenerationPathField.placeholderString = "/v1/images/generations"
        let managedConfigurationLabel = NSTextField(wrappingLabelWithString: AppLocalization.string("providerSettings.protocolsAndEndpointPathsAreDetectedAutomatically"))
        managedConfigurationLabel.textColor = .secondaryLabelColor
        managedConfigurationLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        auxiliaryModelUpstreamButton.toolTip = auxiliaryModelDefaultTooltip()
        let baiduAuthBridgeRow = compactLabeledView(
            AppLocalization.string("providerSettings.authBridge"),
            baiduAuthBridgePopup
        )
        self.baiduAuthBridgeRow = baiduAuthBridgeRow
        baiduCodeReportButton.toolTip =
            "仅对该百度 provider 的会话启用代码使用上报，复用托管 DUCX 的 data-report。"
        let baiduCodeReportRow = compactLabeledView("上报数据", baiduCodeReportButton)
        self.baiduCodeReportRow = baiduCodeReportRow
        let form = NSStackView(views: [
            compactLabeledView("Provider ID", idField),
            customDisplayNameRow,
            customBaseURLRow,
            customWebsiteURLRow,
            compactLabeledView("绘图接口路径", imageGenerationPathField),
            compactLabeledView("API 密钥", apiKeyControls),
            quotaUsernameRow,
            quotaWorkspaceIDRow,
            quotaAuthCookieRow,
            baiduAuthBridgeRow,
            baiduCodeReportRow,
            compactLabeledView("辅助模型", auxiliaryModelUpstreamButton),
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
        baiduAuthBridgePopup.target = self
        baiduAuthBridgePopup.action = #selector(baiduAuthBridgeChanged)
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

        let detailsDocument = FlippedProviderSettingsView()
        detailsDocument.translatesAutoresizingMaskIntoConstraints = false
        detailsDocument.addSubview(detailsPane)
        detailsDocument.addSubview(emptyLabel)

        let detailsScroll = NSScrollView()
        detailsScroll.documentView = detailsDocument
        detailsScroll.hasVerticalScroller = true
        detailsScroll.autohidesScrollers = true
        detailsScroll.drawsBackground = false
        detailsScroll.borderType = .noBorder
        detailsScroll.translatesAutoresizingMaskIntoConstraints = false

        let body = NSSplitView()
        body.isVertical = true
        body.dividerStyle = .thin
        body.translatesAutoresizingMaskIntoConstraints = false
        body.addArrangedSubview(providerPane)
        body.addArrangedSubview(detailsScroll)

        contentView.addSubview(header)
        contentView.addSubview(bannerView)
        contentView.addSubview(body)
        NSLayoutConstraint.activate([
            header.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 24),
            header.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -24),
            header.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 20),

            bannerView.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            bannerView.trailingAnchor.constraint(equalTo: header.trailingAnchor),
            bannerView.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 10),
            bannerHeightConstraint!,
            bannerLabel.leadingAnchor.constraint(equalTo: bannerView.leadingAnchor, constant: 12),
            bannerLabel.trailingAnchor.constraint(equalTo: bannerView.trailingAnchor, constant: -12),
            bannerLabel.centerYAnchor.constraint(equalTo: bannerView.centerYAnchor),

            body.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            body.trailingAnchor.constraint(equalTo: header.trailingAnchor),
            body.topAnchor.constraint(equalTo: bannerView.bottomAnchor, constant: 6),
            body.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -20),
            detailsScroll.widthAnchor.constraint(greaterThanOrEqualToConstant: 520),

            detailsDocument.widthAnchor.constraint(equalTo: detailsScroll.contentView.widthAnchor),
            detailsDocument.heightAnchor.constraint(greaterThanOrEqualTo: detailsScroll.contentView.heightAnchor),
            detailsPane.leadingAnchor.constraint(equalTo: detailsDocument.leadingAnchor, constant: 18),
            detailsPane.trailingAnchor.constraint(equalTo: detailsDocument.trailingAnchor, constant: -18),
            detailsPane.topAnchor.constraint(equalTo: detailsDocument.topAnchor, constant: 12),
            detailsPane.bottomAnchor.constraint(lessThanOrEqualTo: detailsDocument.bottomAnchor, constant: -18),
            form.widthAnchor.constraint(equalTo: detailsPane.widthAnchor),
            actionRow.widthAnchor.constraint(equalTo: detailsPane.widthAnchor),
            statusLabel.widthAnchor.constraint(equalTo: detailsPane.widthAnchor),

            emptyLabel.centerXAnchor.constraint(equalTo: detailsDocument.centerXAnchor),
            emptyLabel.centerYAnchor.constraint(equalTo: detailsDocument.centerYAnchor),
        ])
        setDetailControlsEnabled(false)
    }

    private func showBanner(title: String, message: String, isError: Bool) {
        bannerHideWorkItem?.cancel()
        bannerLabel.stringValue = message.isEmpty ? title : "\(title)：\(message)"
        bannerLabel.textColor = isError ? .mixinError : .labelColor
        let color = isError ? NSColor.mixinError : NSColor.mixinHealthy
        bannerView.layer?.backgroundColor = color.withAlphaComponent(0.12).cgColor
        bannerView.isHidden = false
        bannerHeightConstraint?.constant = 34
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.2
            bannerView.animator().alphaValue = 1
            window?.contentView?.layoutSubtreeIfNeeded()
        }

        let workItem = DispatchWorkItem { [weak self] in
            self?.hideBanner()
        }
        bannerHideWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + 5, execute: workItem)
    }

    private func hideBanner() {
        bannerHideWorkItem?.cancel()
        bannerHideWorkItem = nil
        bannerHeightConstraint?.constant = 0
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            bannerView.animator().alphaValue = 0
            window?.contentView?.layoutSubtreeIfNeeded()
        }, completionHandler: { [weak self] in
            self?.bannerView.isHidden = true
        })
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
        quotaWorkspaceIDField.placeholderString = "例如：wrk_abc123"
        quotaAuthCookieField.placeholderString = "opencode.ai auth cookie"
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
        let auxiliary = provider.auxiliaryModelUpstream ? " · 辅助上游" : ""
        detail?.stringValue = "\(provider.id) · \(readinessLabel(provider.readiness))\(auxiliary) · 已选 \(provider.selectedModels.count) / 可用 \(provider.cachedModels.count)"
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
                codexInstallMode = loaded.codexInstallMode
                providers = loaded.providers
                providerTable.frame = NSRect(
                    x: 0,
                    y: 0,
                    width: providerScrollView.contentSize.width,
                    height: max(
                        providerScrollView.contentSize.height,
                        CGFloat(providers.count) * providerTable.rowHeight
                    )
                )
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
                DispatchQueue.main.async { [weak self] in
                    self?.showBaiduBridgeReminderIfNeeded()
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
        websiteURLField.stringValue = provider.websiteURL ?? ""
        imageGenerationPathField.stringValue = provider.imageGenerationPath ?? ""
        apiKeyField.stringValue = ""
        apiKeyField.placeholderString = provider.apiKeyConfigured
            ? "已配置；留空保留"
            : "尚未配置；启用前必须填写"
        quotaUsernameField.stringValue = provider.quotaUsername ?? ""
        quotaWorkspaceIDField.stringValue = provider.quotaWorkspaceID ?? ""
        quotaAuthCookieField.stringValue = ""
        let openCodeGoQuotaConfigured = provider.quotaAuthCookieConfigured == true
        quotaAuthCookieField.placeholderString = openCodeGoQuotaConfigured
            ? "已配置；留空保留"
            : "opencode.ai auth cookie"
        clearQuotaCredentialsButton.isEnabled = openCodeGoQuotaConfigured
        auxiliaryModelUpstreamButton.state = provider.auxiliaryModelUpstream ? .on : .off
        selectPopupValue(
            baiduAuthBridgePopup,
            provider.effectiveBaiduAuthBridge?.rawValue ?? BaiduAuthBridgeMode.disabled.rawValue
        )
        auxiliaryModelUpstreamButton.toolTip = auxiliaryModelTooltip(for: provider)
        baiduCodeReportButton.state = provider.baiduCodeReport == true ? .on : .off
        let isCustom = provider.presetID == "custom"
        customDisplayNameRow?.isHidden = !isCustom
        customBaseURLRow?.isHidden = !isCustom
        customWebsiteURLRow?.isHidden = !isCustom
        quotaUsernameRow?.isHidden = provider.presetID != "baidu-oneapi"
        let openCodeGo = requiresOpenCodeGoQuotaCredentials(provider.presetID ?? "")
        quotaWorkspaceIDRow?.isHidden = !openCodeGo
        quotaAuthCookieRow?.isHidden = !openCodeGo
        baiduAuthBridgeRow?.isHidden = provider.presetID != "baidu-oneapi"
        baiduCodeReportRow?.isHidden = provider.presetID != "baidu-oneapi"
        enableButton.title = provider.enabled ? "停用" : "启用"
        statusLabel.stringValue = selectedProviderStatus()
        statusLabel.toolTip = provider.lastModelRefreshError
    }

    private func clearDetails() {
        for field in [
            idField,
            displayNameField,
            baseURLField,
            websiteURLField,
            apiKeyField,
            quotaUsernameField,
            quotaWorkspaceIDField,
            quotaAuthCookieField,
        ] {
            field.stringValue = ""
        }
        clearQuotaCredentialsButton.isEnabled = false
        auxiliaryModelUpstreamButton.state = .off
        selectPopupValue(baiduAuthBridgePopup, BaiduAuthBridgeMode.disabled.rawValue)
        baiduCodeReportButton.state = .off
        auxiliaryModelUpstreamButton.toolTip = auxiliaryModelDefaultTooltip()
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
            websiteURLField,
            imageGenerationPathField,
            quotaUsernameField,
            quotaWorkspaceIDField,
            quotaAuthCookieField,
            clearQuotaCredentialsButton,
            auxiliaryModelUpstreamButton,
            baiduAuthBridgePopup,
            enableButton,
            testButton,
            saveButton,
        ]
        for control in controls {
            control.isEnabled = enabled
        }
        auxiliaryModelUpstreamButton.isEnabled = enabled
            && selectedProvider.map {
                isAuxiliaryModelUpstreamSelectable(
                    for: $0,
                    codexInstallMode: codexInstallMode
                )
            } == true
        clearKeyButton.isEnabled = enabled && selectedProvider?.apiKeyConfigured == true
        clearQuotaCredentialsButton.isEnabled =
            enabled && selectedProvider?.quotaAuthCookieConfigured == true
    }

    private func auxiliaryModelDefaultTooltip() -> String {
        switch codexInstallMode {
        case .customOnly:
            return AppLocalization.string("providerSettings.onlyOneAuxiliaryModelProviderCanBe")
        case .codexOAuthProxy:
            return AppLocalization.string("providerSettings.onlyOneAuxiliaryModelProviderCanBe2")
        case nil:
            return AppLocalization.string("providerSettings.onlyOneAuxiliaryModelProviderCanBe3")
        }
    }

    private func auxiliaryModelTooltip(for provider: ProviderView) -> String {
        let capability: String
        switch (codexInstallMode, provider.auxiliaryModelSupport) {
        case (.customOnly, .none):
            capability = AppLocalization.string("providerSettings.thisProviderSupportsNeitherAutoReviewNor")
        case (.customOnly, .autoReviewOnly):
            capability = AppLocalization.string("providerSettings.thisProviderSupportsAutoReviewOnlyVoice")
        case (.customOnly, .voiceOnly):
            capability = AppLocalization.string("providerSettings.thisProviderSupportsVoiceOnlyAutoReview")
        case (.codexOAuthProxy, .none):
            capability = AppLocalization.string("providerSettings.thisProviderOffersNeitherAutoReviewNor")
        case (.codexOAuthProxy, .autoReviewOnly):
            capability = AppLocalization.string("providerSettings.thisProviderOffersAutoReviewOnlyVoice")
        case (.codexOAuthProxy, .voiceOnly):
            capability = AppLocalization.string("providerSettings.thisProviderOffersVoiceOnlyAutoReview")
        case (_, .none):
            capability = AppLocalization.string("providerSettings.theCurrentModelCacheContainsNoAuto")
        case (_, .autoReviewOnly):
            capability = AppLocalization.string("providerSettings.thisProviderSupportsAutoReviewOnly")
        case (_, .voiceOnly):
            capability = AppLocalization.string("providerSettings.thisProviderSupportsVoiceOnly")
        case (_, .autoReviewAndVoice):
            capability = AppLocalization.string("providerSettings.thisProviderSupportsBothAutoReviewAnd")
        }
        return "\(capability)\n\n\(auxiliaryModelDefaultTooltip())"
    }

    private func auxiliaryModelStatus(for provider: ProviderView) -> String? {
        switch (codexInstallMode, provider.auxiliaryModelSupport) {
        case (.customOnly, .none):
            return "辅助模型不可用：自动审查和语音均不支持"
        case (_, .autoReviewOnly):
            return "辅助模型：仅支持自动审查"
        case (_, .voiceOnly):
            return "辅助模型：仅支持语音"
        case (.codexOAuthProxy, .none):
            return "辅助模型：使用 OAuth 默认路由"
        case (_, .none), (_, .autoReviewAndVoice):
            return nil
        }
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
        if let auxiliaryStatus = auxiliaryModelStatus(for: provider) {
            details.insert(auxiliaryStatus, at: 0)
        }
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
            appendProviderArgument(&arguments, "--website-url", values.websiteURL)
        }
        appendProviderArgument(&arguments, "--quota-username", values.quotaUsername)
        if requiresOpenCodeGoQuotaCredentials(values.preset) {
            appendProviderArgument(
                &arguments,
                "--quota-workspace-id",
                values.quotaWorkspaceID
            )
            appendProviderArgument(
                &arguments,
                "--quota-auth-cookie",
                values.quotaAuthCookie
            )
        }
        let bridgeMode = BaiduAuthBridgeMode(rawValue: values.baiduAuthBridge) ?? .disabled
        if values.preset == "baidu-oneapi" {
            appendBaiduAuthBridgeArguments(&arguments, mode: bridgeMode)
        }
        performMutation(
            arguments,
            status: "正在新增并发现模型 \(id)…",
            selecting: id,
            requiresBaiduBridge: values.preset == "baidu-oneapi" ? bridgeMode : nil
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
        guard selectedBaiduAuthBridgeMode() == (provider.effectiveBaiduAuthBridge ?? .disabled) else {
            showAlert(
                title: "认证方式尚未应用",
                message: "先保存认证方式，再测试连接或刷新模型。未保存的下拉框选择不会改变网关配置。"
            )
            return
        }
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

    @objc private func baiduAuthBridgeChanged() {
        guard let provider = selectedProvider, provider.presetID == "baidu-oneapi" else { return }
        let saved = provider.effectiveBaiduAuthBridge ?? .disabled
        if selectedBaiduAuthBridgeMode() == saved {
            statusLabel.stringValue = selectedProviderStatus()
            testButton.isEnabled = !isBusy
        } else {
            statusLabel.stringValue = "认证方式尚未保存；保存后测试连接或刷新模型"
            testButton.isEnabled = false
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

    @objc private func clearQuotaCredentials() {
        guard let provider = selectedProvider, !isBusy else { return }
        guard requiresOpenCodeGoQuotaCredentials(provider.presetID ?? ""),
              provider.quotaAuthCookieConfigured == true
        else {
            return
        }
        guard confirm(
            title: AppLocalization.string("providerSettings.clearOpenCodeGoQuotaCredentials"),
            message: AppLocalization.string("providerSettings.thisRemovesTheWorkspaceIDAndAuth")
        ) else { return }
        performMutation(
            ["providers", "update", provider.id, "--clear-quota"],
            status: "正在清除 \(provider.id) 的额度凭据…",
            selecting: provider.id
        )
    }

    private func showOpenCodeGoQuotaCredentialsAlert() {
        showAlert(
            title: AppLocalization.string("providerSettings.opencodeGoQuotaCredentialsRequired"),
            message: AppLocalization.string("providerSettings.opencodeGoRequiresBothTheWorkspaceID")
        )
    }

    @objc private func saveProvider() {
        guard let provider = selectedProvider, !isBusy else { return }
        let auxiliaryModelUpstream = auxiliaryModelUpstreamButton.state == .on
        var update = ["providers", "update", provider.id]
        update.append("--auxiliary-model-upstream")
        update.append(auxiliaryModelUpstream ? "true" : "false")
        appendProviderArgument(&update, "--key", apiKeyField.stringValue)
        let imageGenerationPath = imageGenerationPathField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if imageGenerationPath.isEmpty {
            if provider.imageGenerationPath != nil {
                update.append("--clear-image-generation")
            }
        } else {
            update.append("--image-generation-path")
            update.append(imageGenerationPath)
        }
        if provider.presetID == "custom" {
            let displayName = displayNameField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            let baseURL = baseURLField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !displayName.isEmpty, !baseURL.isEmpty else {
                showAlert(
                    title: AppLocalization.string("providerSettings.customSiteInformationRequired"),
                    message: AppLocalization.string("providerSettings.siteNameAndAPIURLCannotBe")
                )
                return
            }
            appendProviderArgument(&update, "--display-name", displayName)
            appendProviderArgument(&update, "--base-url", baseURL)
            let websiteURL = websiteURLField.stringValue
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if !websiteURL.isEmpty || provider.websiteURL != nil {
                update.append(contentsOf: ["--website-url", websiteURL])
            }
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
        let selectedBaiduBridge = selectedBaiduAuthBridgeMode()
        if provider.presetID == "baidu-oneapi" {
            appendProviderArgument(&update, "--quota-username", quotaUsername)
            appendBaiduAuthBridgeArguments(&update, mode: selectedBaiduBridge)
            update.append(contentsOf: [
                "--baidu-code-report",
                baiduCodeReportButton.state == .on ? "true" : "false",
            ])
        }
        if requiresOpenCodeGoQuotaCredentials(provider.presetID ?? "") {
            let workspaceID = quotaWorkspaceIDField.stringValue
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let authCookie = quotaAuthCookieField.stringValue
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let credentialsUnchanged =
                workspaceID == (provider.quotaWorkspaceID ?? "") && authCookie.isEmpty
            if !credentialsUnchanged {
                guard !workspaceID.isEmpty, !authCookie.isEmpty else {
                    showOpenCodeGoQuotaCredentialsAlert()
                    return
                }
                appendProviderArgument(&update, "--quota-workspace-id", workspaceID)
                appendProviderArgument(&update, "--quota-auth-cookie", authCookie)
            }
        }
        performMutation(
            update,
            status: "正在保存 \(provider.id)…",
            selecting: provider.id,
            requiresBaiduBridge: provider.presetID == "baidu-oneapi"
                && baiduBridgeNeedsSetup(
                    current: provider.effectiveBaiduAuthBridge,
                    selected: selectedBaiduBridge
                ) ? selectedBaiduBridge : nil,
            codexSkillChanged: auxiliaryModelUpstream != provider.auxiliaryModelUpstream
        )
    }

    private func showBaiduBridgeReminderIfNeeded() {
        guard !isBusy, let window else { return }
        guard let provider = providers.first(where: {
            $0.presetID == "baidu-oneapi"
                && $0.effectiveBaiduAuthBridge == nil
                && !remindedBaiduBridgeProviderIDs.contains($0.id)
        }) else { return }
        remindedBaiduBridgeProviderIDs.insert(provider.id)

        let alert = NSAlert()
        alert.alertStyle = .informational
        alert.messageText = AppLocalization.string("providerSettings.chooseABaiduAuthBridge")
        alert.informativeText = AppLocalization.string("providerSettings.ducxUsesACodexMixinManagedCopy")
        alert.addButton(withTitle: AppLocalization.string("providerSettings.configureDUCX"))
        alert.addButton(withTitle: AppLocalization.string("providerSettings.keepDisabled"))
        baiduBridgeReminderAlert = alert
        alert.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            baiduBridgeReminderAlert = nil
            switch response {
            case .alertFirstButtonReturn:
                configureBaiduBridgeFromReminder(provider, mode: .ducxLoopback)
            default:
                persistBaiduBridgeDisabled(provider.id)
            }
        }
    }

    private func configureBaiduBridgeFromReminder(
        _ provider: ProviderView,
        mode: BaiduAuthBridgeMode
    ) {
        guard !isBusy else { return }
        let name = baiduBridgeDisplayName(mode)
        setBusy(true, status: "正在打开终端配置 \(name)…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: "正在配置 \(name)",
                    phases: [
                        "打开终端完成登录",
                        "写入认证桥接",
                        "重启本地网关",
                        "完成",
                    ],
                    successTitle: "✓ \(name) 已配置",
                    failureTitle: "✗ 配置失败",
                    showFailureAlert: true,
                    failureAlertTitle: "供应商操作失败"
                ) { progress in
                    progress.advance(to: 0)
                    let executable = try await self.ensureBaiduBridgeAvailable(mode)
                    progress.advance(to: 1)
                    var arguments = ["providers", "update", provider.id]
                    self.appendBaiduAuthBridgeArguments(
                        &arguments,
                        mode: mode,
                        executable: executable
                    )
                    _ = try await self.runHandler(arguments)
                    progress.advance(to: 2)
                    self.setBusy(true, status: "正在重启网关并应用 \(name) 配置…")
                    try await self.applyHandler(progress)
                    progress.advance(to: 3)
                    selectPopupValue(self.baiduAuthBridgePopup, mode.rawValue)
                }
                setBusy(false, status: "\(name) 已配置，网关已重启")
                try await Task.sleep(nanoseconds: 350_000_000)
                close()
                let title = AppLocalization.string("providerSettings.configured", name)
                let message = AppLocalization.string(
                    "providerSettings.downloadAndLoginAreCompleteTheProvider",
                    name,
                    name
                )
                if let completionHandler {
                    completionHandler(title, message)
                } else if window != nil {
                    showBanner(title: title, message: message, isError: false)
                } else {
                    showAlert(title: title, message: message)
                }
            } catch {
                setBusy(false, status: "\(name) 配置失败")
                reloadProviders(selecting: provider.id)
            }
        }
    }

    private func persistBaiduBridgeDisabled(_ providerID: String) {
        guard !isBusy else { return }
        setBusy(true, status: "正在保持认证桥接关闭…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: "正在更新认证桥接",
                    phases: [
                        "写入供应商配置",
                        "重启本地网关",
                        "刷新 Codex 模型目录",
                        "完成",
                    ],
                    successTitle: "✓ 认证桥接已关闭",
                    failureTitle: "✗ 操作失败",
                    showFailureAlert: true,
                    failureAlertTitle: "供应商操作失败"
                ) { progress in
                    progress.advance(to: 0)
                    var arguments = ["providers", "update", providerID]
                    self.appendBaiduAuthBridgeArguments(&arguments, mode: .disabled)
                    _ = try await self.runHandler(arguments)
                    try await self.applyHandler(progress)
                }
                setBusy(false, status: "认证桥接保持关闭")
                reloadProviders(selecting: providerID)
            } catch {
                setBusy(false, status: "操作失败")
            }
        }
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
        _ initialArguments: [String],
        then secondArguments: [String]? = nil,
        status: String,
        selecting providerID: String?,
        requiresBaiduBridge: BaiduAuthBridgeMode? = nil,
        codexSkillChanged: Bool = false
    ) {
        guard !isBusy else { return }
        setBusy(true, status: status)
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: "正在更新供应商配置",
                    phases: [
                        "写入供应商配置",
                        "重启本地网关",
                        "刷新 Codex 模型目录",
                        "完成",
                    ],
                    detail: status,
                    successTitle: "✓ 配置已保存",
                    failureTitle: "✗ 操作失败",
                    showFailureAlert: true,
                    failureAlertTitle: "供应商操作失败"
                ) { progress in
                    progress.advance(to: 0)
                    var arguments = initialArguments
                    if let mode = requiresBaiduBridge, mode != .disabled {
                        let executable = try await self.ensureBaiduBridgeAvailable(mode)
                        self.appendBaiduAuthBridgeExecutable(
                            &arguments,
                            mode: mode,
                            executable: executable
                        )
                    }
                    _ = try await self.runHandler(arguments)
                    if let secondArguments {
                        _ = try await self.runHandler(secondArguments)
                    }
                    try await self.applyHandler(progress)
                }
                setBusy(false, status: "配置已保存")
                reloadProviders(selecting: providerID)
                showAlert(
                    title: AppLocalization.string("providerSettings.providerConfigurationUpdated"),
                    message: codexSkillChanged
                        ? "生图 Skill 已更新。请重启 Codex 后新建线程，使 Codex 重新加载 Skill。"
                        : AppLocalization.string("providerSettings.theCodexModelCatalogHasBeenRegenerated")
                )
            } catch {
                setBusy(false, status: "操作失败")
                reloadProviders(selecting: providerID)
            }
        }
    }

    private func selectedBaiduAuthBridgeMode() -> BaiduAuthBridgeMode {
        BaiduAuthBridgeMode(
            rawValue: selectedPopupValue(
                baiduAuthBridgePopup,
                fallback: BaiduAuthBridgeMode.disabled.rawValue
            )
        ) ?? .disabled
    }

    private func appendBaiduAuthBridgeArguments(
        _ arguments: inout [String],
        mode: BaiduAuthBridgeMode,
        executable: URL? = nil
    ) {
        arguments.append(contentsOf: ["--baidu-auth-bridge", mode.rawValue])
        if let executable {
            appendBaiduAuthBridgeExecutable(
                &arguments,
                mode: mode,
                executable: executable
            )
        }
    }

    private func appendBaiduAuthBridgeExecutable(
        _ arguments: inout [String],
        mode: BaiduAuthBridgeMode,
        executable: URL
    ) {
        switch mode {
        case .ducxLoopback:
            arguments.append(contentsOf: ["--ducx-executable", executable.path])
        case .disabled:
            break
        }
    }

    private func ensureBaiduBridgeAvailable(_ mode: BaiduAuthBridgeMode) async throws -> URL {
        if let baiduBridgeSetupHandler {
            return try await baiduBridgeSetupHandler(mode)
        }
        switch mode {
        case .ducxLoopback:
            setBusy(true, status: "请在终端完成 DUCX 下载与扫码登录…")
            return try await setupDucxInTerminal()
        case .disabled:
            throw GatewayError.command("关闭认证桥接不需要安装客户端。")
        }
    }
}

private func baiduBridgeDisplayName(_ mode: BaiduAuthBridgeMode) -> String {
    switch mode {
    case .disabled: return AppLocalization.string("providerSettings.authBridge2")
    case .ducxLoopback: return "DUCX"
    }
}

func managedDucxExecutableURL(
    homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser
) -> URL {
    homeDirectory
        .appendingPathComponent(
            ".codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx",
            isDirectory: false
        )
}

/// Run the managed DUCX install + QR login in a dedicated Terminal via the
/// bundled `codex-mixin connect ducx`, then return the managed executable.
private func setupDucxInTerminal() async throws -> URL {
    guard let cli = Bundle.main.resourceURL?
        .appendingPathComponent("codex-mixin")
    else {
        throw GatewayError.command("无法定位 codex-mixin 可执行文件。")
    }
    let executable = managedDucxExecutableURL()
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("codex-mixin-ducx-setup-\(UUID().uuidString)")
    let script = directory.appendingPathComponent("Configure DUCX.command")
    let loginStatus = directory.appendingPathComponent("login.status")
    let terminalTitle = "Codex Mixin DUCX \(UUID().uuidString)"
    try FileManager.default.createDirectory(
        at: directory,
        withIntermediateDirectories: true,
        attributes: [.posixPermissions: 0o700]
    )
    var setupCompleted = false
    defer {
        if setupCompleted {
            try? FileManager.default.removeItem(at: directory)
        } else {
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 60) {
                try? FileManager.default.removeItem(at: directory)
            }
        }
    }
    let contents = """
    #!/bin/zsh
    printf '\\033]0;\(terminalTitle)\\007'
    echo 'Codex Mixin — DUCX 隔离下载与扫码登录'
    echo '================================'
    \(shellQuoted(cli.path)) connect ducx
    login_result=$?
    printf '%s' "$login_result" > \(shellQuoted(loginStatus.path))
    if [[ "$login_result" -ne 0 ]]; then
      echo
      echo "DUCX 配置失败（退出码 $login_result）。按任意键关闭。"
      read -k 1
      exit "$login_result"
    fi
    echo 'DUCX 配置完成，正在返回 Codex Mixin 应用...'
    (
      sleep 1
      /usr/bin/osascript \\
        -e 'tell application "Terminal"' \\
        -e 'repeat with candidateWindow in windows' \\
        -e 'if name of candidateWindow contains "\(terminalTitle)" then close candidateWindow' \\
        -e 'end repeat' \\
        -e 'end tell'
    ) >/dev/null 2>&1 &!
    exit 0
    """
    try contents.write(to: script, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes(
        [.posixPermissions: 0o700],
        ofItemAtPath: script.path
    )
    guard NSWorkspace.shared.open(script) else {
        throw GatewayError.command("无法打开 Terminal 配置 DUCX。")
    }
    let loginResult = try await waitForBridgeStatus(
        at: loginStatus,
        stage: "DUCX 登录",
        timeoutSeconds: 1_800
    )
    guard loginResult == 0 else {
        throw GatewayError.command("DUCX 配置未成功完成（退出码 \(loginResult)）。")
    }
    guard FileManager.default.isExecutableFile(atPath: executable.path) else {
        throw GatewayError.command("DUCX 配置完成，但托管入口不可执行。")
    }
    setupCompleted = true
    return executable
}

private func waitForBridgeStatus(
    at status: URL,
    stage: String,
    timeoutSeconds: Int
) async throws -> Int32 {
    for _ in 0..<(timeoutSeconds * 4) {
        if let value = try? String(contentsOf: status, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines),
           let result = Int32(value)
        {
            return result
        }
        try await Task.sleep(nanoseconds: 250_000_000)
    }
    throw GatewayError.command("等待\(stage)超时。")
}

private func shellQuoted(_ value: String) -> String {
    "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
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
    row.spacing = 10
    return row
}
