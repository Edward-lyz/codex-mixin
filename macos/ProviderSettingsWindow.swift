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
    private var configuredDetailsViews: [NSView] = []

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
        window.minSize = NSSize(width: 860, height: 600)
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

    func tableView(
        _ tableView: NSTableView,
        writeRowsWith rowIndexes: IndexSet,
        to pasteboard: NSPasteboard
    ) -> Bool {
        guard tableView === providerTable,
              rowIndexes.count == 1,
              let row = rowIndexes.first,
              providers.indices.contains(row),
              providers[row].kind == .configured
        else {
            return false
        }
        pasteboard.clearContents()
        return pasteboard.setString(providers[row].id, forType: .string)
    }

    func tableView(
        _ tableView: NSTableView,
        validateDrop info: NSDraggingInfo,
        proposedRow row: Int,
        proposedDropOperation dropOperation: NSTableView.DropOperation
    ) -> NSDragOperation {
        guard tableView === providerTable,
              dropOperation == .above,
              let providerID = info.draggingPasteboard.string(forType: .string),
              let source = providers.firstIndex(where: { $0.id == providerID }),
              providers[source].kind == .configured
        else {
            return []
        }
        let firstConfigured = providers.firstIndex(where: { $0.kind == .configured }) ?? providers.count
        let insertionRow = min(max(row, firstConfigured), providers.count)
        tableView.setDropRow(insertionRow, dropOperation: .above)
        return .move
    }

    func tableView(
        _ tableView: NSTableView,
        acceptDrop info: NSDraggingInfo,
        row: Int,
        dropOperation: NSTableView.DropOperation
    ) -> Bool {
        guard tableView === providerTable,
              dropOperation == .above,
              let providerID = info.draggingPasteboard.string(forType: .string),
              let source = providers.firstIndex(where: { $0.id == providerID }),
              providers[source].kind == .configured
        else {
            return false
        }
        let firstConfigured = providers.firstIndex(where: { $0.kind == .configured }) ?? providers.count
        var insertion = min(max(row, firstConfigured), providers.count)
        let movedProvider = providers.remove(at: source)
        if source < insertion {
            insertion -= 1
        }
        providers.insert(movedProvider, at: insertion)
        providerTable.reloadData()
        let newRow = providers.firstIndex(where: { $0.id == providerID }) ?? insertion
        providerTable.selectRowIndexes(IndexSet(integer: newRow), byExtendingSelection: false)
        loadSelectedProvider()
        persistProviderOrder(selecting: providerID)
        return true
    }

    private var selectedProvider: ProviderView? {
        let row = providerTable.selectedRow
        return providers.indices.contains(row) ? providers[row] : nil
    }

    private func buildContent(in window: NSWindow) {
        guard let contentView = window.contentView else { return }

        let titleLabel = NSTextField(labelWithString: "供应商设置")
        titleLabel.font = .boldSystemFont(ofSize: 22)
        let detailLabel = NSTextField(labelWithString: "管理连接、密钥和启停状态")
        detailLabel.font = .systemFont(ofSize: 13, weight: .medium)
        detailLabel.textColor = .secondaryLabelColor

        let header = NSStackView(views: [titleLabel, detailLabel])
        header.orientation = .vertical
        header.alignment = .leading
        header.spacing = 4
        header.translatesAutoresizingMaskIntoConstraints = false

        bannerView.wantsLayer = true
        bannerView.layer?.cornerRadius = 10
        bannerView.alphaValue = 0
        bannerView.isHidden = true
        bannerView.translatesAutoresizingMaskIntoConstraints = false
        bannerLabel.font = .systemFont(ofSize: 13, weight: .medium)
        bannerLabel.lineBreakMode = .byTruncatingTail
        bannerLabel.translatesAutoresizingMaskIntoConstraints = false
        bannerView.addSubview(bannerLabel)
        bannerHeightConstraint = bannerView.heightAnchor.constraint(equalToConstant: 0)

        configureProviderTable()
        providerScrollView.documentView = providerTable
        providerScrollView.hasVerticalScroller = true
        providerScrollView.autohidesScrollers = true
        providerScrollView.borderType = .noBorder
        providerScrollView.drawsBackground = false
        providerScrollView.translatesAutoresizingMaskIntoConstraints = false
        providerTable.frame = NSRect(x: 0, y: 0, width: 1, height: 1)
        providerTable.autoresizingMask = [.width]

        let listTitle = NSTextField(labelWithString: "供应商")
        listTitle.font = .systemFont(ofSize: 15, weight: .semibold)
        let listHint = NSTextField(labelWithString: "拖动行调整优先级")
        listHint.font = .systemFont(ofSize: 12)
        listHint.textColor = .secondaryLabelColor
        let listHintIcon = NSImageView(
            image: NSImage(
                systemSymbolName: "arrow.up.arrow.down",
                accessibilityDescription: "调整优先级"
            ) ?? NSImage()
        )
        listHintIcon.contentTintColor = .secondaryLabelColor
        listHintIcon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 12, weight: .medium)
        let listHintStack = NSStackView(views: [listHintIcon, listHint])
        listHintStack.orientation = .horizontal
        listHintStack.alignment = .centerY
        listHintStack.spacing = 5
        let listHeader = NSStackView(views: [listTitle, listHintStack])
        listHeader.orientation = .vertical
        listHeader.alignment = .leading
        listHeader.spacing = 4
        listHeader.translatesAutoresizingMaskIntoConstraints = false

        let providerSurface = providerSettingsSurface(material: .sidebar)
        providerSurface.addSubview(providerScrollView)
        NSLayoutConstraint.activate([
            providerScrollView.leadingAnchor.constraint(equalTo: providerSurface.leadingAnchor),
            providerScrollView.trailingAnchor.constraint(equalTo: providerSurface.trailingAnchor),
            providerScrollView.topAnchor.constraint(equalTo: providerSurface.topAnchor),
            providerScrollView.bottomAnchor.constraint(equalTo: providerSurface.bottomAnchor),
        ])

        configureButton(addButton, action: #selector(addProvider))
        configureButton(removeButton, action: #selector(removeProvider))
        let providerButtons = NSStackView(views: [addButton, removeButton])
        providerButtons.orientation = .horizontal
        providerButtons.distribution = .fillEqually
        providerButtons.spacing = 8

        let providerPane = NSStackView(views: [listHeader, providerSurface, providerButtons])
        providerPane.orientation = .vertical
        providerPane.alignment = .width
        providerPane.spacing = 12
        providerPane.translatesAutoresizingMaskIntoConstraints = false
        providerPane.widthAnchor.constraint(equalToConstant: 252).isActive = true

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
        auxiliaryModelUpstreamButton.toolTip = auxiliaryModelDefaultTooltip(for: codexInstallMode)
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
        ])
        form.orientation = .vertical
        form.alignment = .width
        form.spacing = 11
        form.translatesAutoresizingMaskIntoConstraints = false

        let managedConfigurationIcon = NSImageView(
            image: NSImage(
                systemSymbolName: "wand.and.stars",
                accessibilityDescription: "自动配置"
            ) ?? NSImage()
        )
        managedConfigurationIcon.contentTintColor = .secondaryLabelColor
        managedConfigurationIcon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 13, weight: .medium)
        managedConfigurationLabel.font = .systemFont(ofSize: 12.5)
        let managedConfigurationView = NSStackView(views: [managedConfigurationIcon, managedConfigurationLabel])
        managedConfigurationView.orientation = .horizontal
        managedConfigurationView.alignment = .top
        managedConfigurationView.spacing = 7
        managedConfigurationView.translatesAutoresizingMaskIntoConstraints = false
        managedConfigurationView.setContentHuggingPriority(.required, for: .vertical)
        form.addArrangedSubview(managedConfigurationView)

        let sectionTitle = NSTextField(labelWithString: "连接配置")
        sectionTitle.font = .systemFont(ofSize: 16, weight: .semibold)

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
        configuredDetailsViews = [sectionTitle, form, actionRow]

        statusLabel.font = .systemFont(ofSize: 12.5)
        statusLabel.textColor = .secondaryLabelColor
        statusLabel.maximumNumberOfLines = 2
        statusLabel.lineBreakMode = .byTruncatingTail

        let detailsPane = NSStackView(views: [sectionTitle, form, actionRow, statusLabel])
        detailsPane.orientation = .vertical
        detailsPane.alignment = .width
        detailsPane.spacing = 16
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

        let detailsSurface = providerSettingsSurface()
        detailsSurface.addSubview(detailsScroll)
        NSLayoutConstraint.activate([
            detailsScroll.leadingAnchor.constraint(equalTo: detailsSurface.leadingAnchor),
            detailsScroll.trailingAnchor.constraint(equalTo: detailsSurface.trailingAnchor),
            detailsScroll.topAnchor.constraint(equalTo: detailsSurface.topAnchor),
            detailsScroll.bottomAnchor.constraint(equalTo: detailsSurface.bottomAnchor),
        ])

        let body = NSSplitView()
        body.isVertical = true
        body.dividerStyle = .thin
        body.translatesAutoresizingMaskIntoConstraints = false
        body.addArrangedSubview(providerPane)
        body.addArrangedSubview(detailsSurface)

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
            body.topAnchor.constraint(equalTo: bannerView.bottomAnchor, constant: 16),
            body.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -20),
            detailsSurface.widthAnchor.constraint(greaterThanOrEqualToConstant: 560),

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
        providerTable.rowHeight = 54
        providerTable.allowsMultipleSelection = false
        providerTable.usesAlternatingRowBackgroundColors = false
        providerTable.backgroundColor = .clear
        providerTable.intercellSpacing = NSSize(width: 0, height: 0)
        providerTable.selectionHighlightStyle = .regular
        providerTable.registerForDraggedTypes([.string])
        providerTable.setDraggingSourceOperationMask(.move, forLocal: true)
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
            title.font = .systemFont(ofSize: 14, weight: .semibold)
            title.translatesAutoresizingMaskIntoConstraints = false
            let detail = NSTextField(labelWithString: "")
            detail.font = .systemFont(ofSize: 12)
            detail.textColor = .secondaryLabelColor
            detail.translatesAutoresizingMaskIntoConstraints = false
            detail.lineBreakMode = .byTruncatingTail
            detail.maximumNumberOfLines = 1
            let handle = NSImageView(
                image: NSImage(
                    systemSymbolName: "line.3.horizontal",
                    accessibilityDescription: "拖动排序"
                ) ?? NSImage()
            )
            handle.identifier = NSUserInterfaceItemIdentifier("drag-handle")
            handle.contentTintColor = .tertiaryLabelColor
            handle.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 13, weight: .medium)
            handle.translatesAutoresizingMaskIntoConstraints = false
            let stack = NSStackView(views: [title, detail])
            stack.orientation = .vertical
            stack.alignment = .leading
            stack.spacing = 3
            stack.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(stack)
            cell.addSubview(handle)
            cell.textField = title
            cell.identifier = identifier
            detail.identifier = NSUserInterfaceItemIdentifier("detail")
            NSLayoutConstraint.activate([
                stack.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 7),
                stack.trailingAnchor.constraint(equalTo: handle.leadingAnchor, constant: -8),
                stack.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                title.widthAnchor.constraint(equalTo: stack.widthAnchor),
                detail.widthAnchor.constraint(equalTo: stack.widthAnchor),
                handle.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -10),
                handle.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                handle.widthAnchor.constraint(equalToConstant: 14),
                handle.heightAnchor.constraint(equalToConstant: 18),
            ])
        }
        cell.textField?.stringValue = provider.displayName
        let detail = cell.subviews
            .compactMap { $0 as? NSStackView }
            .flatMap(\.arrangedSubviews)
            .first { $0.identifier?.rawValue == "detail" } as? NSTextField
        if provider.kind == .official {
            detail?.stringValue = "官方 · OAuth 登录 · \(readinessLabel(provider.readiness)) · 只读"
        } else {
            let auxiliary = provider.auxiliaryModelUpstream ? " · 辅助上游" : ""
            detail?.stringValue = "\(provider.id) · \(readinessLabel(provider.readiness))\(auxiliary) · \(provider.selectedModels.count)/\(provider.cachedModels.count) 个模型"
        }
        cell.subviews
            .compactMap { $0 as? NSImageView }
            .first { $0.identifier?.rawValue == "drag-handle" }?
            .isHidden = provider.kind == .official
        return cell
    }

    private func persistProviderOrder(selecting providerID: String) {
        let ids = providers
            .filter { $0.kind == .configured }
            .map(\.id)
        guard !isBusy else { return }
        setBusy(true, status: "正在保存 Provider 顺序…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await runHandler(["providers", "reorder"] + ids)
                setBusy(false, status: "Provider 顺序已保存")
            } catch {
                setBusy(false, status: "Provider 顺序保存失败")
                showAlert(title: "保存 Provider 顺序失败", message: String(describing: error))
                reloadProviders(selecting: providerID)
            }
        }
    }

    private func reloadProviders(selecting providerID: String? = nil) {
        guard !isBusy else { return }
        setBusy(true, status: "正在读取供应商…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                setBusy(
                    false,
                    status: selectedProviderStatus(
                        provider: selectedProvider,
                        providersEmpty: providers.isEmpty,
                        codexInstallMode: codexInstallMode
                    )
                )
            }
            do {
                let previousID = providerID ?? selectedProvider?.id
                let loaded = try await loadHandler()
                codexInstallMode = loaded.codexInstallMode
                providers = loaded.providers
                providerTable.frame = NSRect(
                    x: 0,
                    y: 0,
                    width: max(providerScrollView.contentSize.width, 1),
                    height: max(CGFloat(providers.count) * providerTable.rowHeight, 1)
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
        let isOfficial = provider.kind == .official
        for view in configuredDetailsViews {
            view.isHidden = isOfficial
        }
        idField.stringValue = provider.id
        displayNameField.stringValue = provider.displayName
        baseURLField.stringValue = provider.baseURL
        websiteURLField.stringValue = provider.websiteURL ?? ""
        imageGenerationPathField.stringValue = provider.imageGenerationPath ?? ""
        apiKeyField.stringValue = ""
        apiKeyField.placeholderString = provider.kind == .official
            ? "由 Codex 官方登录管理"
            : provider.apiKeyConfigured
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
        auxiliaryModelUpstreamButton.toolTip = auxiliaryModelTooltip(
            for: provider,
            codexInstallMode: codexInstallMode
        )
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
        enableButton.title = provider.kind == .official
            ? "由登录模式管理"
            : provider.enabled ? "停用" : "启用"
        statusLabel.stringValue = selectedProviderStatus(
            provider: provider,
            providersEmpty: providers.isEmpty,
            codexInstallMode: codexInstallMode
        )
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
        auxiliaryModelUpstreamButton.toolTip = auxiliaryModelDefaultTooltip(for: codexInstallMode)
        statusLabel.stringValue = providers.isEmpty ? "等待新增 Provider" : "请选择 Provider"
        statusLabel.toolTip = nil
    }

    private func setBusy(_ busy: Bool, status: String) {
        isBusy = busy
        statusLabel.stringValue = status
        setDetailControlsEnabled(!busy && selectedProvider != nil)
        addButton.isEnabled = !busy
        removeButton.isEnabled = !busy && selectedProvider?.kind == .configured
    }

    private func setDetailControlsEnabled(_ enabled: Bool) {
        let editable = enabled && selectedProvider?.kind == .configured
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
            control.isEnabled = editable
        }
        auxiliaryModelUpstreamButton.isEnabled = editable
            && selectedProvider.map {
                isAuxiliaryModelUpstreamSelectable(
                    for: $0,
                    codexInstallMode: codexInstallMode
                )
            } == true
        clearKeyButton.isEnabled = editable && selectedProvider?.apiKeyConfigured == true
        clearQuotaCredentialsButton.isEnabled =
            editable && selectedProvider?.quotaAuthCookieConfigured == true
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
        guard let provider = selectedProvider, provider.kind == .configured, !isBusy else { return }
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
        guard let provider = selectedProvider, provider.kind == .configured, !isBusy else { return }
        let action = provider.enabled ? "disable" : "enable"
        performMutation(
            ["providers", action, provider.id],
            status: "正在\(provider.enabled ? "停用" : "启用") \(provider.id)…",
            selecting: provider.id
        )
    }

    @objc private func testProvider() {
        guard let provider = selectedProvider, provider.kind == .configured, !isBusy else { return }
        let selectedBridge = selectedBaiduAuthBridgeMode()
        var arguments = ["providers", "test", provider.id, "--json"]
        appendProviderArgument(&arguments, "--key", apiKeyField.stringValue)
        if provider.presetID == "custom" {
            appendProviderArgument(&arguments, "--base-url", baseURLField.stringValue)
        }
        if provider.presetID == "baidu-oneapi",
           selectedBridge != (provider.effectiveBaiduAuthBridge ?? .disabled) {
            arguments.append(contentsOf: ["--baidu-auth-bridge", selectedBridge.rawValue])
        }
        setBusy(true, status: "正在测试 \(provider.id)…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                setBusy(
                    false,
                    status: selectedProviderStatus(
                        provider: selectedProvider,
                        providersEmpty: providers.isEmpty,
                        codexInstallMode: codexInstallMode
                    )
                )
            }
            do {
                if provider.presetID == "baidu-oneapi",
                   selectedBridge == .ducxLoopback,
                   selectedBridge != (provider.effectiveBaiduAuthBridge ?? .disabled) {
                    let executable = try await ensureBaiduBridgeAvailable(selectedBridge)
                    arguments.append(contentsOf: ["--ducx-executable", executable.path])
                }
                let output = try await runHandler(arguments)
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
            statusLabel.stringValue = selectedProviderStatus(
                provider: provider,
                providersEmpty: providers.isEmpty,
                codexInstallMode: codexInstallMode
            )
        } else {
            statusLabel.stringValue = "认证方式尚未保存；测试连接将使用当前选择，保存后才会写入配置"
        }
        testButton.isEnabled = !isBusy
    }

    @objc private func clearProviderKey() {
        guard let provider = selectedProvider,
              provider.kind == .configured,
              !isBusy,
              provider.apiKeyConfigured
        else { return }
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
        guard let provider = selectedProvider, provider.kind == .configured, !isBusy else { return }
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
        guard let provider = selectedProvider, provider.kind == .configured, !isBusy else { return }
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
                    appendBaiduAuthBridgeArguments(
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
                    appendBaiduAuthBridgeArguments(&arguments, mode: .disabled)
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
                        appendBaiduAuthBridgeExecutable(
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


func compactLabeledView(_ title: String, _ field: NSView) -> NSView {
    let label = NSTextField(labelWithString: title)
    label.alignment = .right
    label.font = .systemFont(ofSize: 13, weight: .medium)
    label.textColor = .secondaryLabelColor
    label.translatesAutoresizingMaskIntoConstraints = false
    label.widthAnchor.constraint(equalToConstant: 112).isActive = true
    label.setContentHuggingPriority(.required, for: .horizontal)
    field.translatesAutoresizingMaskIntoConstraints = false
    field.setContentHuggingPriority(.defaultLow, for: .horizontal)
    field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    let row = NSStackView(views: [label, field])
    row.orientation = .horizontal
    row.alignment = .centerY
    row.distribution = .fill
    row.spacing = 12
    row.setContentHuggingPriority(.defaultLow, for: .horizontal)
    row.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    return row
}
