import Cocoa

private struct ModelBenchmarkTableRow {
    let providerID: String
    let model: ProviderModelListItem
    let result: ModelBenchmarkResult?

    var key: String {
        providerModelSelectionKey(providerID: providerID, modelID: model.id)
    }
}

final class ModelBenchmarkWindowController: NSWindowController, NSWindowDelegate,
    NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate
{
    typealias StartHandler = (Int, String, Int) async throws -> ModelBenchmarkSnapshot
    typealias FetchHandler = () async throws -> ModelBenchmarkSnapshot?
    typealias LoadProvidersHandler = () async throws -> ProviderListResponse
    typealias SaveSelectionsHandler = ([String: [String]], OperationProgress) async throws -> Void
    typealias DiscoverHandler = (String, @escaping (String) -> Void) async throws -> Void

    private let snapshotURL: URL
    private let startHandler: StartHandler
    private let fetchHandler: FetchHandler
    private let loadProvidersHandler: LoadProvidersHandler
    private let saveSelectionsHandler: SaveSelectionsHandler
    private let discoverHandler: DiscoverHandler
    private let probeHandler: DiscoverHandler
    private var pollingTask: Task<Void, Never>?
    private var snapshot: ModelBenchmarkSnapshot?
    private var providers: [ProviderView] = []
    private var rows: [ModelBenchmarkTableRow] = []
    private var resultCache: [String: ModelBenchmarkResult] = [:]
    private var selectedModelKeys: Set<String> = []
    private var savedModelKeys: Set<String> = []
    private var isSavingSelections = false
    private var isLaunchingBenchmark = false
    private var isDiscoveringModels = false
    private var isProbingCapabilities = false

    private let providerPopup = NSPopUpButton()
    private let modePopup = NSPopUpButton()
    private let timeoutPopup = NSPopUpButton()
    private let startButton = NSButton(title: "测速", target: nil, action: nil)
    private let discoverButton = NSButton(title: "刷新模型", target: nil, action: nil)
    private let probeButton = NSButton(title: "探测已加入模型", target: nil, action: nil)
    private let searchField = NSSearchField()
    private let selectionFilterPopup = NSPopUpButton()
    private let selectAllButton = NSButton(title: "全选", target: nil, action: nil)
    private let selectNoneButton = NSButton(title: "全不选", target: nil, action: nil)
    private let saveSelectionButton = NSButton(title: "保存模型选择", target: nil, action: nil)
    private let progressIndicator = NSProgressIndicator()
    private let statusLabel = NSTextField(labelWithString: "请选择 Provider")
    private let summaryLabel = NSTextField(labelWithString: "默认只测试首 token 延迟（TTFT）")
    private let tableView = NSTableView()
    private let emptyLabel = NSTextField(labelWithString: "当前 Provider 没有模型")

    init(
        snapshotURL: URL,
        startHandler: @escaping StartHandler,
        fetchHandler: @escaping FetchHandler,
        loadProvidersHandler: @escaping LoadProvidersHandler,
        saveSelectionsHandler: @escaping SaveSelectionsHandler,
        discoverHandler: @escaping DiscoverHandler,
        probeHandler: @escaping DiscoverHandler
    ) {
        self.snapshotURL = snapshotURL
        self.startHandler = startHandler
        self.fetchHandler = fetchHandler
        self.loadProvidersHandler = loadProvidersHandler
        self.saveSelectionsHandler = saveSelectionsHandler
        self.discoverHandler = discoverHandler
        self.probeHandler = probeHandler
        let visibleFrame = NSScreen.main?.visibleFrame
            ?? NSRect(x: 0, y: 0, width: 1_280, height: 800)
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: modelBenchmarkContentSize(for: visibleFrame)),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "模型选择与测速"
        window.minSize = NSSize(width: 920, height: 520)
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
        loadPersistedSnapshot()
        reloadProviderModels()
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        beginPolling()
    }

    func windowWillClose(_ notification: Notification) {
        pollingTask?.cancel()
        pollingTask = nil
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        rows.count
    }

    func tableView(
        _ tableView: NSTableView,
        viewFor tableColumn: NSTableColumn?,
        row: Int
    ) -> NSView? {
        guard let column = tableColumn, rows.indices.contains(row) else { return nil }
        let modelRow = rows[row]
        if column.identifier.rawValue == "selected" {
            let button = NSButton(
                checkboxWithTitle: "",
                target: self,
                action: #selector(toggleSelectedModel(_:))
            )
            button.state = selectedModelKeys.contains(modelRow.key) ? .on : .off
            button.isEnabled = modelRow.model.isAvailable && !isSavingSelections
            button.identifier = NSUserInterfaceItemIdentifier(modelRow.key)
            let cell = NSTableCellView()
            cell.addSubview(button)
            button.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                button.centerXAnchor.constraint(equalTo: cell.centerXAnchor),
                button.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
            return cell
        }

        let identifier = column.identifier
        let cell: NSTableCellView
        if let reused = tableView.makeView(withIdentifier: identifier, owner: self)
            as? NSTableCellView
        {
            cell = reused
        } else {
            cell = NSTableCellView()
            cell.identifier = identifier
            let field = NSTextField(labelWithString: "")
            field.translatesAutoresizingMaskIntoConstraints = false
            field.lineBreakMode = identifier.rawValue == "model"
                ? .byTruncatingMiddle
                : .byTruncatingTail
            cell.textField = field
            cell.addSubview(field)
            NSLayoutConstraint.activate([
                field.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 7),
                field.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -7),
                field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }
        configureCell(cell, for: modelRow, columnID: identifier.rawValue)
        return cell
    }

    func tableView(
        _ tableView: NSTableView,
        sortDescriptorsDidChange oldDescriptors: [NSSortDescriptor]
    ) {
        rebuildRows()
    }

    func controlTextDidChange(_ obj: Notification) {
        guard let field = obj.object as? NSSearchField, field === searchField else { return }
        rebuildRows()
    }

    private func configureCell(
        _ cell: NSTableCellView,
        for row: ModelBenchmarkTableRow,
        columnID: String
    ) {
        let field = cell.textField
        field?.font = .systemFont(ofSize: 12)
        field?.textColor = .labelColor
        field?.alignment = .left
        cell.toolTip = nil
        switch columnID {
        case "model":
            let displayName = row.model.displayName.flatMap { $0 == row.model.id ? nil : $0 }
            var suffixes: [String] = []
            if row.model.isNew { suffixes.append("新增") }
            if !row.model.isAvailable { suffixes.append("不可用") }
            let suffix = suffixes.isEmpty ? "" : " [\(suffixes.joined(separator: " · "))]"
            field?.stringValue =
                (displayName.map { "\(row.model.id) · \($0)" } ?? row.model.id) + suffix
            field?.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
            field?.textColor = row.model.isAvailable ? .labelColor : .secondaryLabelColor
            cell.toolTip = row.model.description
        case "ttft":
            field?.alignment = .right
            if let result = row.result, result.status != "completed" {
                let measured = result.ttftMs.map { " · \(formatMilliseconds($0))" } ?? ""
                field?.stringValue = "\(resultStatusTitle(result.status))\(measured)"
                field?.textColor = resultStatusColor(result.status)
                cell.toolTip = result.error
            } else if let ttft = row.result?.ttftMs {
                field?.stringValue = formatMilliseconds(ttft)
                field?.textColor = latencyColor(ttft)
            } else {
                field?.stringValue = "-"
                field?.textColor = .secondaryLabelColor
            }
        case "tps":
            field?.alignment = .right
            field?.stringValue = row.result?.tps.map { String(format: "%.1f tok/s", $0) } ?? "-"
            field?.textColor = row.result?.tps == nil ? .secondaryLabelColor : .labelColor
        case "context":
            field?.alignment = .right
            field?.stringValue = row.model.contextWindow.map(formatContextWindow) ?? "-"
        case "ratio":
            field?.alignment = .right
            field?.stringValue = row.model.ratio ?? "-"
            cell.toolTip = row.model.priceType
        case "protocol":
            field?.stringValue = protocolTitle(row.model.protocolID)
            cell.toolTip = row.model.capabilityProbeError
        case "image":
            field?.stringValue = capabilityTitle(row.model.supportsImage)
            cell.toolTip = row.model.capabilityProbeError
        case "tool-search":
            field?.stringValue = capabilityTitle(row.model.supportsToolSearch)
            cell.toolTip = row.model.capabilityProbeError
        case "web-search":
            field?.stringValue = capabilityTitle(row.model.supportsWebSearch)
            cell.toolTip = row.model.capabilityProbeError
        case "function-tools":
            field?.stringValue = capabilityTitle(row.model.supportsFunctionTools)
            cell.toolTip = row.model.capabilityProbeError
        case "thinking":
            field?.stringValue = capabilityTitle(row.model.supportsThinking)
        default:
            field?.stringValue = ""
        }
    }

    private func buildContent(in window: NSWindow) {
        guard let contentView = window.contentView else { return }

        let titleLabel = NSTextField(labelWithString: "模型选择与测速")
        titleLabel.font = .boldSystemFont(ofSize: 20)
        summaryLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        summaryLabel.textColor = .secondaryLabelColor
        let titleStack = NSStackView(views: [titleLabel, summaryLabel])
        titleStack.orientation = .vertical
        titleStack.alignment = .leading
        titleStack.spacing = 3

        providerPopup.target = self
        providerPopup.action = #selector(providerChanged)
        providerPopup.widthAnchor.constraint(equalToConstant: 170).isActive = true

        modePopup.addItem(withTitle: "延迟（TTFT）")
        modePopup.lastItem?.representedObject = 1
        modePopup.addItem(withTitle: "完整（TTFT + 吞吐）")
        modePopup.lastItem?.representedObject = 100
        modePopup.widthAnchor.constraint(equalToConstant: 164).isActive = true

        for seconds in [5, 10, 20, 30, 60] {
            timeoutPopup.addItem(withTitle: "\(seconds) 秒")
            timeoutPopup.lastItem?.representedObject = seconds
        }
        let timeoutDefaultsKey = "modelBenchmarkTimeoutSecondsV2"
        let savedTimeout = UserDefaults.standard.integer(forKey: timeoutDefaultsKey)
        let initialTimeout = [5, 10, 20, 30, 60].contains(savedTimeout) ? savedTimeout : 5
        if let index = timeoutPopup.itemArray.firstIndex(where: {
            ($0.representedObject as? Int) == initialTimeout
        }) {
            timeoutPopup.selectItem(at: index)
        }
        timeoutPopup.target = self
        timeoutPopup.action = #selector(timeoutChanged)
        timeoutPopup.widthAnchor.constraint(equalToConstant: 92).isActive = true

        startButton.bezelStyle = .rounded
        startButton.image = benchmarkSymbol("speedometer")
        startButton.imagePosition = .imageLeading
        startButton.target = self
        startButton.action = #selector(startBenchmark)

        discoverButton.bezelStyle = .rounded
        discoverButton.image = benchmarkSymbol("arrow.clockwise")
        discoverButton.imagePosition = .imageLeading
        discoverButton.target = self
        discoverButton.action = #selector(refreshProviderModels)

        probeButton.bezelStyle = .rounded
        probeButton.image = benchmarkSymbol("waveform.path.ecg")
        probeButton.imagePosition = .imageLeading
        probeButton.target = self
        probeButton.action = #selector(probeSelectedModels)

        let topControls = NSStackView(views: [
            labeledControl("Provider", providerPopup),
            labeledControl("测速", modePopup),
            labeledControl("超时", timeoutPopup),
            discoverButton,
            probeButton,
        ])
        topControls.orientation = .horizontal
        topControls.alignment = .centerY
        topControls.spacing = 12

        let header = NSStackView(views: [titleStack, NSView(), topControls])
        header.orientation = .horizontal
        header.alignment = .centerY
        header.spacing = 16
        header.translatesAutoresizingMaskIntoConstraints = false

        searchField.placeholderString = "搜索当前 Provider 的模型"
        searchField.delegate = self
        searchField.widthAnchor.constraint(greaterThanOrEqualToConstant: 280).isActive = true
        for item in [
            ("全部模型", "all"),
            ("已加入 Codex", "selected"),
            ("新增", "new"),
            ("不可用", "unavailable"),
        ] {
            selectionFilterPopup.addItem(withTitle: item.0)
            selectionFilterPopup.lastItem?.representedObject = item.1
        }
        selectionFilterPopup.target = self
        selectionFilterPopup.action = #selector(selectionFilterChanged)

        for button in [selectAllButton, selectNoneButton, saveSelectionButton] {
            button.bezelStyle = .rounded
            button.target = self
        }
        selectAllButton.action = #selector(selectAllVisibleModels)
        selectNoneButton.action = #selector(selectNoVisibleModels)
        saveSelectionButton.image = benchmarkSymbol("square.and.arrow.down")
        saveSelectionButton.imagePosition = .imageLeading
        saveSelectionButton.action = #selector(saveModelSelections)
        let selectionControls = NSStackView(views: [
            searchField,
            selectionFilterPopup,
            NSView(),
            selectAllButton,
            selectNoneButton,
            saveSelectionButton,
            startButton,
        ])
        selectionControls.orientation = .horizontal
        selectionControls.alignment = .centerY
        selectionControls.spacing = 8
        selectionControls.translatesAutoresizingMaskIntoConstraints = false

        for definition in modelBenchmarkColumnDefinitions() {
            let column = NSTableColumn(
                identifier: NSUserInterfaceItemIdentifier(definition.id)
            )
            column.title = definition.title
            column.width = definition.width
            column.minWidth = definition.minimumWidth
            column.resizingMask = [.userResizingMask, .autoresizingMask]
            column.sortDescriptorPrototype = NSSortDescriptor(
                key: definition.id,
                ascending: definition.defaultAscending
            )
            tableView.addTableColumn(column)
        }
        tableView.delegate = self
        tableView.dataSource = self
        tableView.rowHeight = 30
        tableView.usesAlternatingRowBackgroundColors = true
        tableView.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        tableView.sortDescriptors = [NSSortDescriptor(key: "model", ascending: true)]

        let scrollView = NSScrollView()
        scrollView.documentView = tableView
        configureModelTableScrollView(scrollView)
        scrollView.translatesAutoresizingMaskIntoConstraints = false

        statusLabel.font = .systemFont(ofSize: 13, weight: .medium)
        statusLabel.lineBreakMode = .byTruncatingMiddle
        progressIndicator.style = .bar
        progressIndicator.isIndeterminate = false
        progressIndicator.minValue = 0
        progressIndicator.maxValue = 1
        progressIndicator.doubleValue = 0
        progressIndicator.heightAnchor.constraint(equalToConstant: 6).isActive = true
        let statusStack = NSStackView(views: [statusLabel, progressIndicator])
        statusStack.orientation = .vertical
        statusStack.alignment = .leading
        statusStack.spacing = 6
        statusStack.translatesAutoresizingMaskIntoConstraints = false

        emptyLabel.font = .systemFont(ofSize: 13)
        emptyLabel.textColor = .secondaryLabelColor
        emptyLabel.alignment = .center
        emptyLabel.translatesAutoresizingMaskIntoConstraints = false

        for view in [header, selectionControls, scrollView, statusStack, emptyLabel] {
            contentView.addSubview(view)
        }
        NSLayoutConstraint.activate([
            header.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 24),
            header.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -24),
            header.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 20),

            selectionControls.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            selectionControls.trailingAnchor.constraint(equalTo: header.trailingAnchor),
            selectionControls.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 16),

            scrollView.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: header.trailingAnchor),
            scrollView.topAnchor.constraint(equalTo: selectionControls.bottomAnchor, constant: 8),

            statusStack.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            statusStack.trailingAnchor.constraint(equalTo: header.trailingAnchor),
            statusStack.topAnchor.constraint(equalTo: scrollView.bottomAnchor, constant: 12),
            statusStack.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -18),
            statusLabel.widthAnchor.constraint(equalTo: statusStack.widthAnchor),
            progressIndicator.widthAnchor.constraint(equalTo: statusStack.widthAnchor),

            emptyLabel.centerXAnchor.constraint(equalTo: scrollView.centerXAnchor),
            emptyLabel.centerYAnchor.constraint(equalTo: scrollView.centerYAnchor),
        ])
        applySnapshot(nil)
    }

    @objc private func providerChanged() {
        rebuildRows()
        applySnapshotStatus()
        updateActionState()
    }

    @objc private func timeoutChanged() {
        UserDefaults.standard.set(
            selectedTimeout(),
            forKey: "modelBenchmarkTimeoutSecondsV2"
        )
    }

    @objc private func selectionFilterChanged() {
        rebuildRows()
    }

    @objc private func selectAllVisibleModels() {
        selectedModelKeys.formUnion(rows.filter(\.model.isAvailable).map(\.key))
        rebuildRows()
        updateActionState()
    }

    @objc private func selectNoVisibleModels() {
        selectedModelKeys.subtract(rows.map(\.key))
        rebuildRows()
        updateActionState()
    }

    @objc private func toggleSelectedModel(_ sender: NSButton) {
        guard let key = sender.identifier?.rawValue else { return }
        if sender.state == .on {
            selectedModelKeys.insert(key)
        } else {
            selectedModelKeys.remove(key)
        }
        if tableView.sortDescriptors.first?.key == "selected" {
            rebuildRows()
        }
        updateActionState()
    }

    @objc private func saveModelSelections() {
        guard !isSavingSelections else { return }
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await persistSelectionsIfNeeded()
                presentBenchmarkMessage(
                    title: "模型选择已保存",
                    message: "Codex 模型目录已更新；重启 Codex App 后模型选择器会使用新列表。"
                )
            } catch {
                presentBenchmarkError(
                    title: "保存模型选择失败",
                    message: localizedErrorDescription(error)
                )
            }
        }
    }

    @objc private func startBenchmark() {
        guard let providerID = selectedProviderID() else { return }
        let timeout = selectedTimeout()
        let targetOutputTokens = modePopup.selectedItem?.representedObject as? Int ?? 1
        UserDefaults.standard.set(timeout, forKey: "modelBenchmarkTimeoutSecondsV2")
        isLaunchingBenchmark = true
        updateActionState()
        statusLabel.stringValue = targetOutputTokens == 1
            ? "正在创建延迟测速任务…"
            : "正在创建完整测速任务…"
        statusLabel.textColor = .secondaryLabelColor
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                isLaunchingBenchmark = false
                updateActionState()
            }
            do {
                let snapshot = try await startHandler(
                    timeout,
                    providerID,
                    targetOutputTokens
                )
                applySnapshot(snapshot)
                if window?.isVisible == true {
                    beginPolling()
                }
            } catch {
                presentBenchmarkError(
                    title: "启动测速失败",
                    message: localizedErrorDescription(error)
                )
                await refreshFromGateway()
            }
        }
    }

    @objc private func refreshProviderModels() {
        guard let providerID = selectedProviderID(), !isDiscoveringModels else { return }
        let providerName = selectedProvider()?.displayName ?? providerID
        isDiscoveringModels = true
        updateActionState()
        statusLabel.stringValue = "正在刷新 \(providerName) 的模型…"
        statusLabel.textColor = .secondaryLabelColor
        progressIndicator.isIndeterminate = true
        progressIndicator.startAnimation(nil)
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                isDiscoveringModels = false
                updateActionState()
            }
            do {
                try await discoverHandler(providerID) { [weak self] progress in
                    guard let self else { return }
                    self.statusLabel.stringValue = localizedProgressLabel(progress)
                    self.statusLabel.textColor = .secondaryLabelColor
                    if let counts = modelCapabilityProbeCounts(progress) {
                        self.progressIndicator.stopAnimation(nil)
                        self.progressIndicator.isIndeterminate = false
                        self.progressIndicator.maxValue = Double(counts.total)
                        self.progressIndicator.doubleValue = Double(counts.completed)
                    }
                }
                reloadProviderModels()
                progressIndicator.stopAnimation(nil)
                progressIndicator.isIndeterminate = false
                progressIndicator.maxValue = 1
                progressIndicator.doubleValue = 1
                statusLabel.stringValue = "已刷新 \(providerName) 的模型"
                statusLabel.textColor = .mixinHealthy
            } catch {
                progressIndicator.stopAnimation(nil)
                progressIndicator.isIndeterminate = false
                progressIndicator.maxValue = 1
                progressIndicator.doubleValue = 0
                presentBenchmarkError(
                    title: "刷新模型失败",
                    message: localizedErrorDescription(error)
                )
            }
        }
    }

    @objc private func probeSelectedModels() {
        guard let providerID = selectedProvider(), !isProbingCapabilities else { return }
        isProbingCapabilities = true
        updateActionState()
        statusLabel.stringValue = "正在探测 \(providerID.displayName) 已加入 Codex 的模型…"
        statusLabel.textColor = .secondaryLabelColor
        progressIndicator.isIndeterminate = true
        progressIndicator.startAnimation(nil)
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                isProbingCapabilities = false
                updateActionState()
            }
            do {
                try await probeHandler(providerID.id) { [weak self] progress in
                    guard let self else { return }
                    self.statusLabel.stringValue = localizedProgressLabel(progress)
                    self.statusLabel.textColor = .secondaryLabelColor
                    if let counts = modelCapabilityProbeCounts(progress) {
                        self.progressIndicator.stopAnimation(nil)
                        self.progressIndicator.isIndeterminate = false
                        self.progressIndicator.maxValue = Double(counts.total)
                        self.progressIndicator.doubleValue = Double(counts.completed)
                    }
                }
                reloadProviderModels()
                progressIndicator.stopAnimation(nil)
                progressIndicator.isIndeterminate = false
                progressIndicator.maxValue = 1
                progressIndicator.doubleValue = 1
                statusLabel.stringValue = "已完成 \(providerID.displayName) 已加入模型的能力探测"
                statusLabel.textColor = .mixinHealthy
            } catch {
                progressIndicator.stopAnimation(nil)
                progressIndicator.isIndeterminate = false
                progressIndicator.maxValue = 1
                progressIndicator.doubleValue = 0
                presentBenchmarkError(
                    title: "探测模型能力失败",
                    message: localizedErrorDescription(error)
                )
            }
        }
    }

    private func selectedProviderID() -> String? {
        providerPopup.selectedItem?.representedObject as? String
    }

    private func selectedProvider() -> ProviderView? {
        guard let providerID = selectedProviderID() else { return nil }
        return providers.first { $0.id == providerID }
    }

    private func selectedTimeout() -> Int {
        timeoutPopup.selectedItem?.representedObject as? Int ?? 5
    }

    private func reloadProviderModels() {
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let response = try await loadProvidersHandler()
                providers = response.providers
                selectedModelKeys = selectedProviderModelKeys(providers)
                savedModelKeys = selectedModelKeys
                setProviderOptions(configuredProviderOptions(providers))
                rebuildRows()
                applySnapshotStatus()
                updateActionState()
            } catch {
                providers = []
                rows = []
                tableView.reloadData()
                emptyLabel.isHidden = false
                presentBenchmarkError(
                    title: "读取模型列表失败",
                    message: localizedErrorDescription(error)
                )
            }
        }
    }

    private func setProviderOptions(_ options: [ProviderPickerOption]) {
        let previousID = selectedProviderID()
        providerPopup.removeAllItems()
        for option in options {
            providerPopup.addItem(withTitle: option.displayName)
            providerPopup.lastItem?.representedObject = option.id
            providerPopup.lastItem?.toolTip = option.id
        }
        if let previousID,
           let index = providerPopup.itemArray.firstIndex(where: {
               ($0.representedObject as? String) == previousID
           })
        {
            providerPopup.selectItem(at: index)
        } else if !options.isEmpty {
            providerPopup.selectItem(at: 0)
        }
    }

    private func rebuildRows() {
        guard let provider = selectedProvider() else {
            rows = []
            tableView.reloadData()
            emptyLabel.isHidden = false
            return
        }
        let query = searchField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let filter = selectionFilterPopup.selectedItem?.representedObject as? String ?? "all"
        rows = provider.modelItems.compactMap { model in
            let key = providerModelSelectionKey(providerID: provider.id, modelID: model.id)
            let matchesQuery = query.isEmpty
                || model.id.localizedCaseInsensitiveContains(query)
                || model.displayName?.localizedCaseInsensitiveContains(query) == true
                || model.description?.localizedCaseInsensitiveContains(query) == true
            let matchesFilter: Bool
            switch filter {
            case "selected":
                matchesFilter = selectedModelKeys.contains(key)
            case "new":
                matchesFilter = model.isNew
            case "unavailable":
                matchesFilter = !model.isAvailable
            default:
                matchesFilter = true
            }
            guard matchesQuery && matchesFilter else { return nil }
            return ModelBenchmarkTableRow(
                providerID: provider.id,
                model: model,
                result: resultCache[key]
            )
        }
        sortRows()
        tableView.reloadData()
        emptyLabel.isHidden = !rows.isEmpty
        updateActionState()
    }

    private func sortRows() {
        guard let descriptor = tableView.sortDescriptors.first, let key = descriptor.key else {
            return
        }
        let ascending = descriptor.ascending
        rows.sort { left, right in
            let comparison = compareRows(left, right, key: key)
            if comparison == .orderedSame {
                return left.model.id.localizedStandardCompare(right.model.id)
                    == .orderedAscending
            }
            return ascending
                ? comparison == .orderedAscending
                : comparison == .orderedDescending
        }
    }

    private func compareRows(
        _ left: ModelBenchmarkTableRow,
        _ right: ModelBenchmarkTableRow,
        key: String
    ) -> ComparisonResult {
        switch key {
        case "selected":
            return compareOptionalNumbers(
                selectedModelKeys.contains(left.key) ? 1 : 0,
                selectedModelKeys.contains(right.key) ? 1 : 0
            )
        case "ttft":
            return compareOptionalNumbers(left.result?.ttftMs, right.result?.ttftMs)
        case "tps":
            return compareOptionalNumbers(left.result?.tps, right.result?.tps)
        case "context":
            return compareOptionalNumbers(left.model.contextWindow, right.model.contextWindow)
        case "ratio":
            return compareOptionalNumbers(
                benchmarkRatioValue(left.model.ratio),
                benchmarkRatioValue(right.model.ratio)
            )
        case "protocol":
            return (left.model.protocolID ?? "~").localizedStandardCompare(
                right.model.protocolID ?? "~"
            )
        case "image":
            return compareOptionalNumbers(
                left.model.supportsImage.map { $0 ? 1 : 0 },
                right.model.supportsImage.map { $0 ? 1 : 0 }
            )
        case "tool-search":
            return compareOptionalNumbers(
                left.model.supportsToolSearch.map { $0 ? 1 : 0 },
                right.model.supportsToolSearch.map { $0 ? 1 : 0 }
            )
        case "web-search":
            return compareOptionalNumbers(
                left.model.supportsWebSearch.map { $0 ? 1 : 0 },
                right.model.supportsWebSearch.map { $0 ? 1 : 0 }
            )
        case "function-tools":
            return compareOptionalNumbers(
                left.model.supportsFunctionTools.map { $0 ? 1 : 0 },
                right.model.supportsFunctionTools.map { $0 ? 1 : 0 }
            )
        case "thinking":
            return compareOptionalNumbers(
                left.model.supportsThinking.map { $0 ? 1 : 0 },
                right.model.supportsThinking.map { $0 ? 1 : 0 }
            )
        default:
            return left.model.id.localizedStandardCompare(right.model.id)
        }
    }

    private func updateActionState() {
        let dirty = selectedModelKeys != savedModelKeys
        let providerSelectedCount = rowsForSelectedProvider().filter {
            selectedModelKeys.contains($0.key) && $0.model.isAvailable
        }.count
        let busy = isSavingSelections || isLaunchingBenchmark || isDiscoveringModels || isProbingCapabilities
            || snapshot?.status == "running"
        saveSelectionButton.isEnabled = dirty && !busy
        selectAllButton.isEnabled = !busy && selectedProvider() != nil
        selectNoneButton.isEnabled = !busy && selectedProvider() != nil
        startButton.isEnabled = !busy && !dirty && providerSelectedCount > 0
        startButton.toolTip = dirty ? "请先保存模型选择" : "测试当前 Provider"
        discoverButton.isEnabled = !busy && selectedProvider() != nil
        probeButton.isEnabled = !busy && !dirty && providerSelectedCount > 0
        providerPopup.isEnabled = !busy
        modePopup.isEnabled = !busy
        timeoutPopup.isEnabled = !busy
    }

    private func rowsForSelectedProvider() -> [ModelBenchmarkTableRow] {
        guard let provider = selectedProvider() else { return [] }
        return provider.modelItems.map { model in
            let key = providerModelSelectionKey(providerID: provider.id, modelID: model.id)
            return ModelBenchmarkTableRow(
                providerID: provider.id,
                model: model,
                result: resultCache[key]
            )
        }
    }

    private func persistSelectionsIfNeeded() async throws {
        guard selectedModelKeys != savedModelKeys else { return }
        isSavingSelections = true
        updateActionState()
        defer {
            isSavingSelections = false
            updateActionState()
        }
        statusLabel.stringValue = "正在保存模型选择并更新 Codex 目录…"
        let selections = providerModelSelections(providers, selectedKeys: selectedModelKeys)
        try await runOperationProgress(
            title: "正在保存模型选择",
            phases: [
                "写入模型选择",
                "重启本地网关",
                "刷新 Codex 模型目录",
                "完成",
            ],
            successTitle: "✓ 模型选择已保存",
            failureTitle: "✗ 保存失败",
            showFailureAlert: false
        ) { progress in
            try await self.saveSelectionsHandler(selections, progress)
        }
        savedModelKeys = selectedModelKeys
    }

    private func beginPolling() {
        pollingTask?.cancel()
        pollingTask = Task { @MainActor [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                await refreshFromGateway()
                if snapshot?.status != "running" { return }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }
    }

    private func refreshFromGateway() async {
        do {
            if let remote = try await fetchHandler() {
                applySnapshot(remote)
            } else {
                loadPersistedSnapshot()
            }
        } catch {
            loadPersistedSnapshot()
            if snapshot?.status == "running" {
                statusLabel.stringValue = "网关状态暂不可用，显示已保存进度"
                statusLabel.textColor = .mixinDegraded
            }
        }
    }

    private func loadPersistedSnapshot() {
        guard FileManager.default.fileExists(atPath: snapshotURL.path) else {
            applySnapshot(nil)
            return
        }
        do {
            let data = try Data(contentsOf: snapshotURL)
            applySnapshot(try JSONDecoder().decode(ModelBenchmarkSnapshot.self, from: data))
        } catch {
            snapshot = nil
            statusLabel.stringValue = "测速结果文件无法读取"
            statusLabel.textColor = .mixinError
            summaryLabel.stringValue = snapshotURL.path
            progressIndicator.doubleValue = 0
            updateActionState()
        }
    }

    private func applySnapshot(_ snapshot: ModelBenchmarkSnapshot?) {
        self.snapshot = snapshot
        if let snapshot {
            for result in snapshot.results {
                let key = providerModelSelectionKey(
                    providerID: result.providerID,
                    modelID: result.upstreamModel
                )
                resultCache[key] = mergedBenchmarkResult(
                    result,
                    previous: resultCache[key],
                    targetOutputTokens: snapshot.targetOutputTokens
                )
            }
        }
        rebuildRows()
        applySnapshotStatus()
        updateActionState()
    }

    private func applySnapshotStatus() {
        guard !isDiscoveringModels else { return }
        guard let snapshot else {
            statusLabel.stringValue = "尚无测速结果"
            statusLabel.textColor = .secondaryLabelColor
            summaryLabel.stringValue = "默认只测试首 token 延迟（TTFT）"
            progressIndicator.maxValue = 1
            progressIndicator.doubleValue = 0
            return
        }
        let selectedProviderID = selectedProviderID()
        let providerResults = snapshot.results.filter {
            selectedProviderID == nil || $0.providerID == selectedProviderID
        }
        progressIndicator.maxValue = Double(max(snapshot.totalModels, 1))
        progressIndicator.doubleValue = Double(snapshot.results.count)
        let mode = snapshot.targetOutputTokens == 1 ? "延迟" : "完整"
        summaryLabel.stringValue =
            "\(formatBenchmarkDate(snapshot.startedAt)) · \(mode)测速 · 超时 \(snapshot.timeoutSeconds) 秒"
        let completed = providerResults.filter { $0.status == "completed" }.count
        let timedOut = providerResults.filter { $0.status == "timed_out" }.count
        let failed = providerResults.filter { $0.status == "failed" }.count
        switch snapshot.status {
        case "running":
            let index = min(snapshot.results.count + 1, snapshot.totalModels)
            statusLabel.stringValue =
                "正在测试 \(snapshot.currentModel ?? "下一模型")（\(index) / \(snapshot.totalModels)）"
            statusLabel.textColor = .controlAccentColor
        case "completed":
            if providerResults.isEmpty {
                statusLabel.stringValue = "当前 Provider 尚未测速"
                statusLabel.textColor = .secondaryLabelColor
            } else {
                statusLabel.stringValue =
                    "测速完成：成功 \(completed)，超时 \(timedOut)，失败 \(failed)"
                statusLabel.textColor = failed == 0 && timedOut == 0
                    ? .mixinHealthy
                    : .mixinDegraded
            }
        case "interrupted":
            statusLabel.stringValue =
                "上次测速已中断，已保存 \(snapshot.results.count) / \(snapshot.totalModels) 个结果"
            statusLabel.textColor = .mixinDegraded
        case "failed":
            statusLabel.stringValue = "测速任务失败：\(snapshot.error ?? "未知错误")"
            statusLabel.textColor = .mixinError
        default:
            statusLabel.stringValue = snapshot.status
            statusLabel.textColor = .secondaryLabelColor
        }
    }
}

private func labeledControl(_ title: String, _ control: NSView) -> NSStackView {
    let label = NSTextField(labelWithString: title)
    label.textColor = .secondaryLabelColor
    let stack = NSStackView(views: [label, control])
    stack.orientation = .horizontal
    stack.alignment = .centerY
    stack.spacing = 6
    return stack
}

private func compareOptionalNumbers<T: BinaryInteger>(
    _ left: T?,
    _ right: T?
) -> ComparisonResult {
    compareOptionalNumbers(left.map(Double.init), right.map(Double.init))
}

private func capabilityTitle(_ supported: Bool?) -> String {
    guard let supported else { return "-" }
    return supported ? "支持" : "不支持"
}

private func protocolTitle(_ protocolID: String?) -> String {
    switch protocolID {
    case "open_ai_responses": return "Responses"
    case "open_ai_chat": return "Chat"
    case "anthropic_messages": return "Messages"
    default: return "-"
    }
}

private func compareOptionalNumbers(
    _ left: Double?,
    _ right: Double?
) -> ComparisonResult {
    switch (left, right) {
    case (nil, nil): return .orderedSame
    case (nil, _): return .orderedDescending
    case (_, nil): return .orderedAscending
    case let (left?, right?):
        if left == right { return .orderedSame }
        return left < right ? .orderedAscending : .orderedDescending
    }
}

private func benchmarkSymbol(_ name: String) -> NSImage? {
    guard #available(macOS 11.0, *) else { return nil }
    let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)
    image?.isTemplate = true
    return image
}

private func formatMilliseconds(_ milliseconds: UInt64) -> String {
    if milliseconds < 1_000 { return "\(milliseconds) ms" }
    return String(format: "%.2f s", Double(milliseconds) / 1_000)
}

private func latencyColor(_ milliseconds: UInt64) -> NSColor {
    switch milliseconds {
    case ..<1_000: return .mixinHealthy
    case ..<3_000: return .mixinDegraded
    default: return .mixinError
    }
}

private func resultStatusTitle(_ status: String) -> String {
    switch status {
    case "completed": return "完成"
    case "timed_out": return "超时"
    case "failed": return "失败"
    default: return status
    }
}

private func resultStatusColor(_ status: String) -> NSColor {
    switch status {
    case "completed": return .mixinHealthy
    case "timed_out": return .mixinDegraded
    case "failed": return .mixinError
    default: return .secondaryLabelColor
    }
}

private func formatBenchmarkDate(_ milliseconds: UInt64) -> String {
    let formatter = DateFormatter()
    formatter.dateStyle = .medium
    formatter.timeStyle = .short
    return formatter.string(from: Date(timeIntervalSince1970: Double(milliseconds) / 1_000))
}

private func presentBenchmarkError(title: String, message: String) {
    let alert = NSAlert()
    alert.messageText = localizedPrompt(title)
    alert.informativeText = localizedGatewayMessage(message)
    alert.alertStyle = .warning
    alert.addButton(withTitle: AppLocalization.string("modelBenchmark.ok"))
    NSApp.activate(ignoringOtherApps: true)
    alert.runModal()
}

private func presentBenchmarkMessage(title: String, message: String) {
    let alert = NSAlert()
    alert.messageText = localizedPrompt(title)
    alert.informativeText = localizedGatewayMessage(message)
    alert.alertStyle = .informational
    alert.addButton(withTitle: AppLocalization.string("modelBenchmark.ok2"))
    NSApp.activate(ignoringOtherApps: true)
    alert.runModal()
}
