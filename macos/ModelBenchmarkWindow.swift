import Cocoa

struct ModelBenchmarkSnapshotEnvelope: Decodable {
    let snapshot: ModelBenchmarkSnapshot?
}

struct ModelBenchmarkSnapshot: Decodable {
    let runId: String
    let status: String
    let startedAt: UInt64
    let updatedAt: UInt64
    let finishedAt: UInt64?
    let timeoutSeconds: UInt64
    let targetOutputTokens: UInt64
    let totalModels: Int
    let currentModel: String?
    let results: [ModelBenchmarkResult]
    let error: String?
    let estimatedCost: Double?
    let costCurrency: String?
    let costError: String?

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case status
        case startedAt = "started_at"
        case updatedAt = "updated_at"
        case finishedAt = "finished_at"
        case timeoutSeconds = "timeout_seconds"
        case targetOutputTokens = "target_output_tokens"
        case totalModels = "total_models"
        case currentModel = "current_model"
        case results
        case error
        case estimatedCost = "estimated_cost"
        case costCurrency = "cost_currency"
        case costError = "cost_error"
    }
}

struct ModelBenchmarkResult: Decodable {
    let model: String
    let status: String
    let ttftMs: UInt64?
    let generationMs: UInt64?
    let totalMs: UInt64
    let outputTokens: UInt64?
    let tps: Double?
    let error: String?

    enum CodingKeys: String, CodingKey {
        case model
        case status
        case ttftMs = "ttft_ms"
        case generationMs = "generation_ms"
        case totalMs = "total_ms"
        case outputTokens = "output_tokens"
        case tps
        case error
    }
}

final class ModelBenchmarkWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate {
    typealias StartHandler = (Int) async throws -> ModelBenchmarkSnapshot
    typealias FetchHandler = () async throws -> ModelBenchmarkSnapshot?

    private let snapshotURL: URL
    private let startHandler: StartHandler
    private let fetchHandler: FetchHandler
    private var pollingTask: Task<Void, Never>?
    private var snapshot: ModelBenchmarkSnapshot?
    private var displayedResults: [ModelBenchmarkResult] = []

    private let timeoutPopup = NSPopUpButton()
    private let startButton = NSButton(title: "开始测速", target: nil, action: nil)
    private let progressIndicator = NSProgressIndicator()
    private let statusLabel = NSTextField(labelWithString: "尚无测速结果")
    private let summaryLabel = NSTextField(labelWithString: "")
    private let costLabel = NSTextField(labelWithString: "")
    private let tableView = NSTableView()
    private let emptyLabel = NSTextField(labelWithString: "暂无测速结果")

    init(snapshotURL: URL, startHandler: @escaping StartHandler, fetchHandler: @escaping FetchHandler) {
        self.snapshotURL = snapshotURL
        self.startHandler = startHandler
        self.fetchHandler = fetchHandler
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 920, height: 560),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "模型测速"
        window.minSize = NSSize(width: 760, height: 420)
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
        displayedResults.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let column = tableColumn, displayedResults.indices.contains(row) else { return nil }
        let result = displayedResults[row]
        let identifier = column.identifier
        let cell: NSTableCellView
        if let reused = tableView.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView {
            cell = reused
        } else {
            cell = NSTableCellView()
            cell.identifier = identifier
            let field = NSTextField(labelWithString: "")
            field.lineBreakMode = identifier.rawValue == "model" ? .byTruncatingMiddle : .byTruncatingTail
            field.translatesAutoresizingMaskIntoConstraints = false
            cell.textField = field
            cell.addSubview(field)
            NSLayoutConstraint.activate([
                field.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
                field.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
                field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }

        let text: String
        switch identifier.rawValue {
        case "model":
            text = result.model
            cell.textField?.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        case "ttft":
            text = result.ttftMs.map(formatMilliseconds) ?? "-"
        case "tps":
            text = result.tps.map { String(format: "%.1f tok/s", $0) } ?? "-"
            cell.toolTip = result.tps != nil && result.generationMs == nil
                ? "上游一次性返回全部输出，TPS 按 output tokens / 请求总耗时计算。"
                : nil
        case "tokens":
            if let outputTokens = result.outputTokens, let target = snapshot?.targetOutputTokens {
                text = "\(outputTokens) / \(target)"
            } else {
                text = "-"
            }
        case "total":
            text = formatMilliseconds(result.totalMs)
        case "result":
            text = resultStatusTitle(result.status)
            cell.textField?.textColor = resultStatusColor(result.status)
            cell.toolTip = result.error
        default:
            text = ""
        }
        if identifier.rawValue != "model" {
            cell.textField?.font = .systemFont(ofSize: 12)
        }
        if identifier.rawValue != "result" {
            cell.textField?.textColor = .labelColor
            if identifier.rawValue != "tps" {
                cell.toolTip = nil
            }
        }
        cell.textField?.stringValue = text
        return cell
    }

    func tableView(_ tableView: NSTableView, sortDescriptorsDidChange oldDescriptors: [NSSortDescriptor]) {
        updateDisplayedResults()
        tableView.reloadData()
    }

    private func buildContent(in window: NSWindow) {
        guard let contentView = window.contentView else { return }

        let titleLabel = NSTextField(labelWithString: "模型测速")
        titleLabel.font = .boldSystemFont(ofSize: 20)

        summaryLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        summaryLabel.textColor = .secondaryLabelColor
        summaryLabel.lineBreakMode = .byTruncatingTail

        costLabel.font = .systemFont(ofSize: 13, weight: .medium)
        costLabel.lineBreakMode = .byTruncatingTail

        timeoutPopup.removeAllItems()
        for seconds in [10, 20, 30, 60] {
            timeoutPopup.addItem(withTitle: "\(seconds) 秒")
            timeoutPopup.lastItem?.representedObject = seconds
        }
        let savedTimeout = UserDefaults.standard.integer(forKey: "modelBenchmarkTimeoutSeconds")
        let initialTimeout = [10, 20, 30, 60].contains(savedTimeout) ? savedTimeout : 10
        if let index = timeoutPopup.itemArray.firstIndex(where: { ($0.representedObject as? Int) == initialTimeout }) {
            timeoutPopup.selectItem(at: index)
        }
        timeoutPopup.target = self
        timeoutPopup.action = #selector(timeoutChanged)
        timeoutPopup.translatesAutoresizingMaskIntoConstraints = false
        timeoutPopup.widthAnchor.constraint(equalToConstant: 96).isActive = true

        let timeoutLabel = NSTextField(labelWithString: "单模型超时")
        timeoutLabel.textColor = .secondaryLabelColor

        startButton.bezelStyle = .rounded
        startButton.image = benchmarkSymbol("speedometer")
        startButton.imagePosition = .imageLeading
        startButton.target = self
        startButton.action = #selector(startBenchmark)
        startButton.translatesAutoresizingMaskIntoConstraints = false
        startButton.widthAnchor.constraint(equalToConstant: 118).isActive = true

        let controls = NSStackView(views: [timeoutLabel, timeoutPopup, startButton])
        controls.orientation = .horizontal
        controls.alignment = .centerY
        controls.spacing = 10

        let headerLeft = NSStackView(views: [titleLabel, summaryLabel, costLabel])
        headerLeft.orientation = .vertical
        headerLeft.alignment = .leading
        headerLeft.spacing = 4

        let header = NSStackView(views: [headerLeft, NSView(), controls])
        header.orientation = .horizontal
        header.alignment = .centerY
        header.spacing = 12
        header.translatesAutoresizingMaskIntoConstraints = false

        statusLabel.font = .systemFont(ofSize: 13, weight: .medium)
        statusLabel.lineBreakMode = .byTruncatingMiddle
        statusLabel.translatesAutoresizingMaskIntoConstraints = false

        progressIndicator.style = .bar
        progressIndicator.isIndeterminate = false
        progressIndicator.minValue = 0
        progressIndicator.maxValue = 1
        progressIndicator.doubleValue = 0
        progressIndicator.translatesAutoresizingMaskIntoConstraints = false
        progressIndicator.heightAnchor.constraint(equalToConstant: 8).isActive = true

        let statusStack = NSStackView(views: [statusLabel, progressIndicator])
        statusStack.orientation = .vertical
        statusStack.alignment = .leading
        statusStack.spacing = 7
        statusStack.translatesAutoresizingMaskIntoConstraints = false

        let columns: [(id: String, title: String, width: CGFloat)] = [
            ("model", "模型", 300),
            ("ttft", "TTFT", 105),
            ("tps", "TPS", 115),
            ("tokens", "Usage / 上限", 110),
            ("total", "总耗时", 100),
            ("result", "结果", 100),
        ]
        for definition in columns {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(definition.id))
            column.title = definition.title
            column.width = definition.width
            column.minWidth = definition.id == "model" ? 180 : 80
            column.resizingMask = [.userResizingMask, .autoresizingMask]
            column.sortDescriptorPrototype = NSSortDescriptor(key: definition.id, ascending: true)
            tableView.addTableColumn(column)
        }
        tableView.delegate = self
        tableView.dataSource = self
        tableView.rowHeight = 30
        tableView.usesAlternatingRowBackgroundColors = true
        tableView.allowsMultipleSelection = false
        tableView.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle

        let scrollView = NSScrollView()
        scrollView.documentView = tableView
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = false
        scrollView.autohidesScrollers = true
        scrollView.borderType = .bezelBorder
        scrollView.translatesAutoresizingMaskIntoConstraints = false

        emptyLabel.font = .systemFont(ofSize: 13)
        emptyLabel.textColor = .secondaryLabelColor
        emptyLabel.alignment = .center
        emptyLabel.translatesAutoresizingMaskIntoConstraints = false

        contentView.addSubview(header)
        contentView.addSubview(statusStack)
        contentView.addSubview(scrollView)
        contentView.addSubview(emptyLabel)
        NSLayoutConstraint.activate([
            header.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 24),
            header.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -24),
            header.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 22),
            summaryLabel.widthAnchor.constraint(lessThanOrEqualToConstant: 480),

            statusStack.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            statusStack.trailingAnchor.constraint(equalTo: header.trailingAnchor),
            statusStack.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 18),
            statusLabel.widthAnchor.constraint(equalTo: statusStack.widthAnchor),
            progressIndicator.widthAnchor.constraint(equalTo: statusStack.widthAnchor),

            scrollView.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 24),
            scrollView.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -24),
            scrollView.topAnchor.constraint(equalTo: statusStack.bottomAnchor, constant: 18),
            scrollView.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -22),

            emptyLabel.centerXAnchor.constraint(equalTo: scrollView.centerXAnchor),
            emptyLabel.centerYAnchor.constraint(equalTo: scrollView.centerYAnchor),
        ])
        applySnapshot(nil)
    }

    @objc private func timeoutChanged() {
        UserDefaults.standard.set(selectedTimeout(), forKey: "modelBenchmarkTimeoutSeconds")
    }

    @objc private func startBenchmark() {
        let timeout = selectedTimeout()
        UserDefaults.standard.set(timeout, forKey: "modelBenchmarkTimeoutSeconds")
        startButton.isEnabled = false
        timeoutPopup.isEnabled = false
        statusLabel.stringValue = "正在创建测速任务..."
        statusLabel.textColor = .secondaryLabelColor
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let snapshot = try await startHandler(timeout)
                applySnapshot(snapshot)
                if window?.isVisible == true {
                    beginPolling()
                }
            } catch {
                startButton.isEnabled = true
                timeoutPopup.isEnabled = true
                presentBenchmarkError(title: "启动测速失败", message: String(describing: error))
                await refreshFromGateway()
            }
        }
    }

    private func selectedTimeout() -> Int {
        timeoutPopup.selectedItem?.representedObject as? Int ?? 10
    }

    private func beginPolling() {
        pollingTask?.cancel()
        pollingTask = Task { @MainActor [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                await refreshFromGateway()
                if snapshot?.status != "running" {
                    return
                }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }
    }

    private func refreshFromGateway() async {
        do {
            let remote = try await fetchHandler()
            if let remote {
                applySnapshot(remote)
            } else {
                loadPersistedSnapshot()
            }
        } catch {
            loadPersistedSnapshot()
            if snapshot?.status == "running" {
                statusLabel.stringValue = "网关状态暂不可用，显示已保存进度"
                statusLabel.textColor = .systemOrange
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
            displayedResults = []
            tableView.reloadData()
            emptyLabel.isHidden = false
            statusLabel.stringValue = "测速结果文件无法读取"
            statusLabel.textColor = .systemRed
            summaryLabel.stringValue = snapshotURL.path
            startButton.isEnabled = true
            timeoutPopup.isEnabled = true
        }
    }

    private func applySnapshot(_ snapshot: ModelBenchmarkSnapshot?) {
        self.snapshot = snapshot
        updateDisplayedResults()
        tableView.reloadData()
        emptyLabel.isHidden = !displayedResults.isEmpty
        guard let snapshot else {
            statusLabel.stringValue = "尚无测速结果"
            statusLabel.textColor = .secondaryLabelColor
            summaryLabel.stringValue = "单模型超时默认 10 秒"
            costLabel.stringValue = "测速完成后显示本次费用"
            costLabel.textColor = .secondaryLabelColor
            costLabel.toolTip = nil
            progressIndicator.maxValue = 1
            progressIndicator.doubleValue = 0
            startButton.isEnabled = true
            timeoutPopup.isEnabled = true
            return
        }

        progressIndicator.maxValue = Double(max(snapshot.totalModels, 1))
        progressIndicator.doubleValue = Double(snapshot.results.count)
        let running = snapshot.status == "running"
        startButton.isEnabled = !running
        timeoutPopup.isEnabled = !running
        summaryLabel.stringValue = "\(formatBenchmarkDate(snapshot.startedAt)) · 单模型超时 \(snapshot.timeoutSeconds) 秒 · 请求上限 \(snapshot.targetOutputTokens) tokens"
        if let cost = snapshot.estimatedCost {
            switch snapshot.costCurrency {
            case "CNY":
                costLabel.stringValue = String(format: "本次测试花费约 ¥%.2f", cost)
            case "USD":
                costLabel.stringValue = String(format: "本次测试花费约 $%.4f", cost)
            default:
                costLabel.stringValue = String(format: "本次测试额度消耗约 %.4f", cost)
            }
            costLabel.textColor = .systemGreen
            costLabel.toolTip = "根据测速前后的已用额度差计算，期间其他请求可能计入结果。"
        } else if snapshot.status == "running" {
            costLabel.stringValue = "本次测试费用统计中"
            costLabel.textColor = .secondaryLabelColor
            costLabel.toolTip = nil
        } else if let costError = snapshot.costError {
            costLabel.stringValue = "本次测试费用不可用"
            costLabel.textColor = .systemOrange
            costLabel.toolTip = costError
        } else {
            costLabel.stringValue = "上次测速未记录费用"
            costLabel.textColor = .secondaryLabelColor
            costLabel.toolTip = nil
        }

        let completed = snapshot.results.filter { $0.status == "completed" }.count
        let timedOut = snapshot.results.filter { $0.status == "timed_out" }.count
        let failed = snapshot.results.filter { $0.status == "failed" }.count
        switch snapshot.status {
        case "running":
            let index = min(snapshot.results.count + 1, snapshot.totalModels)
            statusLabel.stringValue = "正在测试 \(snapshot.currentModel ?? "下一模型")（\(index) / \(snapshot.totalModels)）"
            statusLabel.textColor = .controlAccentColor
        case "completed":
            statusLabel.stringValue = "测速完成：成功 \(completed)，超时 \(timedOut)，失败 \(failed)"
            statusLabel.textColor = failed == 0 && timedOut == 0 ? .systemGreen : .systemOrange
        case "interrupted":
            statusLabel.stringValue = "上次测速已中断，已保存 \(snapshot.results.count) / \(snapshot.totalModels) 个结果"
            statusLabel.textColor = .systemOrange
        case "failed":
            statusLabel.stringValue = "测速任务失败：\(snapshot.error ?? "未知错误")"
            statusLabel.textColor = .systemRed
        default:
            statusLabel.stringValue = snapshot.status
            statusLabel.textColor = .secondaryLabelColor
        }
    }

    private func updateDisplayedResults() {
        let results = snapshot?.results ?? []
        guard
            let descriptor = tableView.sortDescriptors.first,
            let key = descriptor.key
        else {
            displayedResults = results
            return
        }
        let ascending = descriptor.ascending
        displayedResults = results.sorted { left, right in
            if key == "model" || key == "result" {
                let leftValue = key == "model" ? left.model : left.status
                let rightValue = key == "model" ? right.model : right.status
                let comparison = leftValue.localizedStandardCompare(rightValue)
                if comparison == .orderedSame {
                    return left.model.localizedStandardCompare(right.model) == .orderedAscending
                }
                return ascending ? comparison == .orderedAscending : comparison == .orderedDescending
            }

            let leftValue: Double?
            let rightValue: Double?
            switch key {
            case "ttft":
                leftValue = left.ttftMs.map(Double.init)
                rightValue = right.ttftMs.map(Double.init)
            case "tps":
                leftValue = left.tps
                rightValue = right.tps
            case "tokens":
                leftValue = left.outputTokens.map(Double.init)
                rightValue = right.outputTokens.map(Double.init)
            case "total":
                leftValue = Double(left.totalMs)
                rightValue = Double(right.totalMs)
            default:
                return false
            }
            switch (leftValue, rightValue) {
            case (nil, nil):
                return left.model.localizedStandardCompare(right.model) == .orderedAscending
            case (nil, _):
                return false
            case (_, nil):
                return true
            case let (leftValue?, rightValue?):
                if leftValue == rightValue {
                    return left.model.localizedStandardCompare(right.model) == .orderedAscending
                }
                return ascending ? leftValue < rightValue : leftValue > rightValue
            }
        }
    }
}

private func benchmarkSymbol(_ name: String) -> NSImage? {
    guard #available(macOS 11.0, *) else { return nil }
    let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)
    image?.isTemplate = true
    return image
}

private func formatMilliseconds(_ milliseconds: UInt64) -> String {
    if milliseconds < 1_000 {
        return "\(milliseconds) ms"
    }
    return String(format: "%.2f s", Double(milliseconds) / 1_000)
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
    case "completed": return .systemGreen
    case "timed_out": return .systemOrange
    case "failed": return .systemRed
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
    alert.messageText = title
    alert.informativeText = message
    alert.alertStyle = .warning
    alert.addButton(withTitle: "确定")
    NSApp.activate(ignoringOtherApps: true)
    alert.runModal()
}
