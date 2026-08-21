import Cocoa
import SwiftUI

struct ModelBenchmarkTableRow: Identifiable {
    let providerID: String
    let model: ProviderModelListItem
    let result: ModelBenchmarkResult?

    var id: String {
        providerModelSelectionKey(providerID: providerID, modelID: model.id)
    }

    var isSelectedSortValue: Int { 0 }
    var ttftSortValue: UInt64? { result?.ttftMs }
    var tpsSortValue: Double? { result?.tps }
    var contextSortValue: UInt64? { model.contextWindow }
    var ratioSortValue: Double? { benchmarkRatioValue(model.ratio) }
    var protocolSortValue: String? { model.protocolID }
    var imageSortValue: Int? { model.supportsImage.map { $0 ? 1 : 0 } }
    var toolSearchSortValue: Int? { model.supportsToolSearch.map { $0 ? 1 : 0 } }
    var webSearchSortValue: Int? { model.supportsWebSearch.map { $0 ? 1 : 0 } }
    var functionToolsSortValue: Int? { model.supportsFunctionTools.map { $0 ? 1 : 0 } }
    var thinkingSortValue: Int? { model.supportsThinking.map { $0 ? 1 : 0 } }
}

@MainActor
final class ModelBenchmarkModel: ObservableObject {
    let startHandler: (Int, String, Int) async throws -> ModelBenchmarkSnapshot
    let fetchHandler: () async throws -> ModelBenchmarkSnapshot?
    let loadProvidersHandler: () async throws -> ProviderListResponse
    let saveSelectionsHandler: ([String: [String]], OperationProgress) async throws -> Void
    let discoverHandler: (String, @escaping (String) -> Void) async throws -> Void
    let probeHandler: (String, @escaping (String) -> Void) async throws -> Void

    @Published var providers: [ProviderView] = []
    @Published var selectedProviderID: String?
    @Published var visibleRows: [ModelBenchmarkTableRow] = []
    @Published var selectedModelKeys: Set<String> = []
    @Published var savedModelKeys: Set<String> = []
    @Published var query = ""
    @Published var selectionFilter = "all"
    @Published var modeSelection = 1
    @Published var timeoutSeconds = 5
    @Published var statusTitle = "请选择 Provider"
    @Published var statusColor = Color(nsColor: .secondaryLabelColor)
    @Published var summary = "默认只测试首 token 延迟（TTFT）"
    @Published var determinateProgress: Double?
    @Published var snapshot: ModelBenchmarkSnapshot?
    @Published var isSavingSelections = false
    @Published var isLaunchingBenchmark = false
    @Published var isDiscoveringModels = false
    @Published var isProbingCapabilities = false

    private(set) var resultCache: [String: ModelBenchmarkResult] = [:]
    private var pollingTask: Task<Void, Never>?

    init(
        startHandler: @escaping (Int, String, Int) async throws -> ModelBenchmarkSnapshot,
        fetchHandler: @escaping () async throws -> ModelBenchmarkSnapshot?,
        loadProvidersHandler: @escaping () async throws -> ProviderListResponse,
        saveSelectionsHandler: @escaping ([String: [String]], OperationProgress) async throws -> Void,
        discoverHandler: @escaping (String, @escaping (String) -> Void) async throws -> Void,
        probeHandler: @escaping (String, @escaping (String) -> Void) async throws -> Void
    ) {
        self.startHandler = startHandler
        self.fetchHandler = fetchHandler
        self.loadProvidersHandler = loadProvidersHandler
        self.saveSelectionsHandler = saveSelectionsHandler
        self.discoverHandler = discoverHandler
        self.probeHandler = probeHandler
    }

    var selectedProvider: ProviderView? {
        providers.first { $0.id == selectedProviderID }
    }

    var providerOptions: [ProviderPickerOption] {
        configuredProviderOptions(providers)
    }

    var dirty: Bool {
        selectedModelKeys != savedModelKeys
    }

    var isBusy: Bool {
        isSavingSelections || isLaunchingBenchmark || isDiscoveringModels || isProbingCapabilities
            || snapshot?.status == "running"
    }

    var selectedVisibleCount: Int {
        visibleRows.filter { selectedModelKeys.contains($0.id) && $0.model.isAvailable }.count
    }

    func stopPolling() {
        pollingTask?.cancel()
        pollingTask = nil
    }

    func resetForPresentation() {
        stopPolling()
        resultCache.removeAll()
        applySnapshot(nil)
        reloadProviders()
    }

    func selectProvider(_ providerID: String?) {
        selectedProviderID = providerID
        rebuildRows()
    }

    func isSelected(_ row: ModelBenchmarkTableRow) -> Bool {
        selectedModelKeys.contains(row.id)
    }

    func setSelected(_ row: ModelBenchmarkTableRow, isSelected: Bool) {
        guard row.model.isAvailable, !isBusy else { return }
        if isSelected {
            selectedModelKeys.insert(row.id)
        } else {
            selectedModelKeys.remove(row.id)
        }
    }

    func selectAllVisible() {
        selectedModelKeys.formUnion(visibleRows.filter(\.model.isAvailable).map(\.id))
    }

    func selectNoneVisible() {
        selectedModelKeys.subtract(visibleRows.map(\.id))
    }

    func saveSelections() {
        guard dirty, !isBusy else { return }
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

    func startBenchmark() {
        guard let providerID = selectedProviderID, !isBusy, !dirty else { return }
        let timeout = timeoutSeconds
        let targetOutputTokens = modeSelection
        UserDefaults.standard.set(timeout, forKey: "modelBenchmarkTimeoutSecondsV2")
        isLaunchingBenchmark = true
        setStatus(targetOutputTokens == 1 ? "正在创建延迟测速任务…" : "正在创建完整测速任务…", color: .secondary)
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                isLaunchingBenchmark = false
            }
            do {
                let loadedSnapshot = try await startHandler(timeout, providerID, targetOutputTokens)
                applySnapshot(loadedSnapshot)
                beginPolling()
            } catch {
                presentBenchmarkError(
                    title: "启动测速失败",
                    message: localizedErrorDescription(error)
                )
                await refreshFromGateway()
            }
        }
    }

    func refreshModels() {
        guard let providerID = selectedProviderID, !isBusy else { return }
        let providerName = selectedProvider?.displayName ?? providerID
        isDiscoveringModels = true
        resetProgress()
        setStatus("正在刷新 \(providerName) 的模型…", color: .secondary)
        runStreaming {
            try await self.discoverHandler(providerID) { progress in
                Task { @MainActor in
                    self.handleProgress(progress)
                }
            }
        } onSuccess: {
            self.reloadProviders(selecting: providerID)
            self.resetProgress(to: 1)
            self.setStatus("已刷新 \(providerName) 的模型", color: .green)
        } onFailure: { error in
            self.resetProgress(to: 0)
            self.presentFailure(title: "刷新模型失败", error: error)
        }
    }

    func probeCapabilities() {
        guard let provider = selectedProvider, !isBusy else { return }
        isProbingCapabilities = true
        resetProgress()
        setStatus("正在探测 \(provider.displayName) 已加入 Codex 的模型…", color: .secondary)
        runStreaming {
            try await self.probeHandler(provider.id) { progress in
                Task { @MainActor in
                    self.handleProgress(progress)
                }
            }
        } onSuccess: {
            self.reloadProviders(selecting: provider.id)
            self.resetProgress(to: 1)
            self.setStatus("已完成 \(provider.displayName) 已加入模型的能力探测", color: .green)
        } onFailure: { error in
            self.resetProgress(to: 0)
            self.presentFailure(title: "探测模型能力失败", error: error)
        }
    }

    func reloadProviders(selecting providerID: String? = nil) {
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let response = try await loadProvidersHandler()
                providers = response.providers
                selectedProviderID = providerID ?? selectedProviderID
                    ?? providers.first(where: { $0.kind == .configured })?.id
                selectedModelKeys = selectedProviderModelKeys(providers)
                savedModelKeys = selectedModelKeys
                rebuildRows()
            } catch {
                providers = []
                visibleRows = []
                selectedProviderID = nil
                selectedModelKeys = []
                savedModelKeys = []
                setStatus("读取模型列表失败", color: .red)
                presentBenchmarkError(
                    title: "读取模型列表失败",
                    message: localizedErrorDescription(error)
                )
            }
        }
    }

    func rebuildRows(
        queryOverride: String? = nil,
        filterOverride: String? = nil,
        sortOverride: [KeyPathComparator<ModelBenchmarkTableRow>]? = nil
    ) {
        guard let provider = selectedProvider else {
            visibleRows = []
            return
        }
        let searchText = (queryOverride ?? query).trimmingCharacters(in: .whitespacesAndNewlines)
        let filter = filterOverride ?? selectionFilter
        let rows = provider.modelItems.compactMap { model -> ModelBenchmarkTableRow? in
            let key = providerModelSelectionKey(providerID: provider.id, modelID: model.id)
            let matchesQuery = searchText.isEmpty
                || model.id.localizedCaseInsensitiveContains(searchText)
                || model.displayName?.localizedCaseInsensitiveContains(searchText) == true
                || model.description?.localizedCaseInsensitiveContains(searchText) == true
            let matchesFilter: Bool
            switch filter {
            case "selected": matchesFilter = selectedModelKeys.contains(key)
            case "new": matchesFilter = model.isNew
            case "unavailable": matchesFilter = !model.isAvailable
            default: matchesFilter = true
            }
            guard matchesQuery && matchesFilter else { return nil }
            return ModelBenchmarkTableRow(
                providerID: provider.id,
                model: model,
                result: resultCache[key]
            )
        }
        visibleRows = rows.sorted(using: sortOverride ?? [
            KeyPathComparator(\ModelBenchmarkTableRow.model.id, comparator: .localizedStandard),
        ])
    }

    private func persistSelectionsIfNeeded() async throws {
        guard dirty else { return }
        isSavingSelections = true
        defer {
            isSavingSelections = false
        }
        setStatus("正在保存模型选择并更新 Codex 目录…", color: .secondary)
        let selections = providerModelSelections(providers, selectedKeys: selectedModelKeys)
        try await runOperationProgress(
            title: "正在保存模型选择",
            phases: ["写入模型选择", "重启本地网关", "刷新 Codex 模型目录", "完成"],
            successTitle: "✓ 模型选择已保存",
            failureTitle: "✗ 保存失败",
            showFailureAlert: false
        ) { progress in
            try await self.saveSelectionsHandler(selections, progress)
        }
        savedModelKeys = selectedModelKeys
    }

    private func beginPolling() {
        stopPolling()
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
            }
        } catch {
            if snapshot?.status == "running" {
                setStatus("网关状态暂不可用，显示已保存进度", color: .orange)
            }
        }
    }

    private func applySnapshot(_ loadedSnapshot: ModelBenchmarkSnapshot?) {
        snapshot = loadedSnapshot
        if let loadedSnapshot {
            for result in loadedSnapshot.results {
                let key = providerModelSelectionKey(
                    providerID: result.providerID,
                    modelID: result.upstreamModel
                )
                resultCache[key] = mergedBenchmarkResult(
                    result,
                    previous: resultCache[key],
                    targetOutputTokens: loadedSnapshot.targetOutputTokens
                )
            }
        }
        rebuildRows()
        applySnapshotStatus()
    }

    private func applySnapshotStatus() {
        guard !isDiscoveringModels else { return }
        guard let currentSnapshot = snapshot else {
            setStatus("尚无测速结果", color: .secondary)
            summary = "默认只测试首 token 延迟（TTFT）"
            determinateProgress = 0
            return
        }
        let providerResults = currentSnapshot.results.filter {
            selectedProviderID == nil || $0.providerID == selectedProviderID
        }
        determinateProgress = Double(currentSnapshot.results.count) / Double(max(currentSnapshot.totalModels, 1))
        let mode = currentSnapshot.targetOutputTokens == 1 ? "延迟" : "完整"
        summary = "\(formatBenchmarkDate(currentSnapshot.startedAt)) · \(mode)测速 · 超时 \(currentSnapshot.timeoutSeconds) 秒"
        let completed = providerResults.filter { $0.status == "completed" }.count
        let timedOut = providerResults.filter { $0.status == "timed_out" }.count
        let failed = providerResults.filter { $0.status == "failed" }.count
        switch currentSnapshot.status {
        case "running":
            let index = min(currentSnapshot.results.count + 1, currentSnapshot.totalModels)
            setStatus(
                "正在测试 \(currentSnapshot.currentModel ?? "下一模型")（\(index) / \(currentSnapshot.totalModels)）",
                color: .accentColor
            )
        case "completed":
            if providerResults.isEmpty {
                setStatus("当前 Provider 尚未测速", color: .secondary)
            } else {
                setStatus(
                    "测速完成：成功 \(completed)，超时 \(timedOut)，失败 \(failed)",
                    color: failed == 0 && timedOut == 0 ? .green : .orange
                )
            }
        case "interrupted":
            setStatus(
                "上次测速已中断，已保存 \(currentSnapshot.results.count) / \(currentSnapshot.totalModels) 个结果",
                color: .orange
            )
        case "failed":
            setStatus("测速任务失败：\(currentSnapshot.error ?? "未知错误")", color: .red)
        default:
            setStatus(currentSnapshot.status, color: .secondary)
        }
    }

    private func runStreaming(
        _ operation: @escaping () async throws -> Void,
        onSuccess: @escaping () -> Void,
        onFailure: @escaping (Error) -> Void
    ) {
        // The handler itself reports streaming progress; completion is centralized here.
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                isDiscoveringModels = false
                isProbingCapabilities = false
            }
            do {
                try await operation()
                onSuccess()
            } catch {
                onFailure(error)
            }
        }
    }

    private func presentFailure(title: String, error: Error) {
        presentBenchmarkError(title: title, message: localizedErrorDescription(error))
    }

    private func setStatus(_ title: String, color: Color) {
        statusTitle = title
        statusColor = color
    }

    private func resetProgress(to value: Double? = nil) {
        determinateProgress = value
    }

    private func handleProgress(_ rawProgress: String) {
        statusTitle = localizedProgressLabel(rawProgress)
        statusColor = .secondary
        if let counts = modelCapabilityProbeCounts(rawProgress) {
            determinateProgress = Double(counts.completed) / Double(counts.total)
        }
    }

}

struct ModelBenchmarkRootView: View {
    @ObservedObject var model: ModelBenchmarkModel
    @State private var sortOrder: [KeyPathComparator<ModelBenchmarkTableRow>] = [
        KeyPathComparator(\ModelBenchmarkTableRow.model.id, comparator: .localizedStandard),
    ]

    var body: some View {
        VStack(spacing: 0) {
            benchmarkToolbar
            selectionToolbar

            modelTableArea
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color.clear)

            Divider()
            statusBar
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .onAppear {
            model.rebuildRows(sortOverride: sortOrder)
        }
        .onChange(of: sortOrder) { _, newValue in
            model.rebuildRows(sortOverride: newValue)
        }
        .onChange(of: model.query) {
            model.rebuildRows(sortOverride: sortOrder)
        }
        .onChange(of: model.selectionFilter) {
            model.rebuildRows(sortOverride: sortOrder)
        }
    }

    @ViewBuilder
    private var modelTableArea: some View {
        if model.visibleRows.isEmpty {
            modelTableEmptyState
        } else {
            modelTable
        }
    }

    private var modelTableEmptyState: some View {
        VStack(spacing: 10) {
            Image(systemName: "cpu")
                .font(.system(size: 30, weight: .medium))
                .foregroundStyle(.tertiary)
            Text(emptyStateTitle)
                .font(.title3.weight(.semibold))
                .foregroundStyle(.secondary)
        }
    }

    private var emptyStateTitle: String {
        model.selectedProvider == nil
            ? "没有可用的自定义 Provider"
            : "当前 Provider 没有模型"
    }

    private var modelTable: some View {
        Table(model.visibleRows, sortOrder: $sortOrder) {
            selectedColumn
            modelColumn
            latencyColumn
            throughputColumn
            contextColumn
            ratioColumn
            protocolColumn
            capabilityColumns
        }
        .scrollContentBackground(.hidden)
    }

    private var providerBinding: Binding<String?> {
        Binding(
            get: { model.selectedProviderID },
            set: { model.selectProvider($0) }
        )
    }

    private var searchBinding: Binding<String> {
        Binding(
            get: { model.query },
            set: { model.query = $0 }
        )
    }

    private var filterBinding: Binding<String> {
        Binding(
            get: { model.selectionFilter },
            set: { model.selectionFilter = $0 }
        )
    }

    private var benchmarkToolbar: some View {
        HStack(spacing: 10) {
            Picker("Provider", selection: providerBinding) {
                ForEach(model.providerOptions, id: \.id) { option in
                    Text(option.displayName).tag(Optional(option.id))
                }
            }
            .labelsHidden()
            .frame(width: 190)

            Picker("测速模式", selection: modeBinding) {
                Text("延迟（TTFT）").tag(1)
                Text("完整（TTFT + 吞吐）").tag(100)
            }
            .labelsHidden()
            .frame(width: 180)

            Picker("超时", selection: timeoutBinding) {
                ForEach([5, 10, 20, 30, 60], id: \.self) { seconds in
                    Text("\(seconds) 秒").tag(seconds)
                }
            }
            .labelsHidden()
            .frame(width: 96)

            Button(action: model.refreshModels) {
                Label("刷新模型", systemImage: "arrow.clockwise")
            }
            .disabled(model.isBusy || model.selectedProvider == nil)

            Button(action: model.probeCapabilities) {
                Label("探测已加入模型", systemImage: "waveform.path.ecg")
            }
            .disabled(model.isBusy || model.dirty || model.selectedVisibleCount == 0)

            Spacer()

            Button(action: model.startBenchmark) {
                Label("测速", systemImage: "speedometer")
            }
            .keyboardShortcut(.defaultAction)
            .buttonStyle(.borderedProminent)
            .disabled(model.isBusy || model.dirty || model.selectedVisibleCount == 0)
        }
        .padding(.horizontal, 20)
        .padding(.top, 18)
        .padding(.bottom, 12)
    }

    private var selectionToolbar: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(.secondary)

            TextField("搜索当前 Provider 的模型", text: searchBinding)
                .textFieldStyle(.roundedBorder)

            Picker("筛选", selection: filterBinding) {
                Text("全部模型").tag("all")
                Text("已加入 Codex").tag("selected")
                Text("新增").tag("new")
                Text("不可用").tag("unavailable")
            }
            .labelsHidden()
            .frame(width: 150)

            Button("全选", action: model.selectAllVisible)
                .disabled(model.isBusy || model.visibleRows.isEmpty)

            Button("全不选", action: model.selectNoneVisible)
                .disabled(model.isBusy || model.visibleRows.isEmpty)

            Button {
                model.saveSelections()
            } label: {
                Label("保存模型选择", systemImage: "square.and.arrow.down")
            }
            .buttonStyle(.borderedProminent)
            .disabled(!model.dirty || model.isBusy)
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 12)
    }

    private var modeBinding: Binding<Int> {
        Binding(
            get: { model.modeSelection },
            set: { model.modeSelection = $0 }
        )
    }

    private var timeoutBinding: Binding<Int> {
        Binding(
            get: { model.timeoutSeconds },
            set: { model.timeoutSeconds = $0 }
        )
    }

    private var selectedColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("加入 Codex", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.isSelectedSortValue)) { row in
            Toggle("", isOn: selectionBinding(row))
                .labelsHidden()
                .disabled(!row.model.isAvailable || model.isBusy)
        }
        .width(min: 82, ideal: 92)
    }

    private var modelColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn(
            "上游模型",
            sortUsing: KeyPathComparator(
                \ModelBenchmarkTableRow.model.id,
                comparator: .localizedStandard
            )
        ) { row in
            modelCell(row)
        }
        .width(min: 280, ideal: 460)
    }

    private var latencyColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("TTFT", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.ttftSortValue)) { row in
            latencyCell(row)
        }
        .width(min: 84, ideal: 104)
        .alignment(.trailing)
    }

    private var throughputColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("吞吐", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.tpsSortValue)) { row in
            throughputCell(row)
        }
        .width(min: 90, ideal: 112)
        .alignment(.trailing)
    }

    private var contextColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("上下文", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.contextSortValue)) { row in
            Text(row.model.contextWindow.map(formatContextWindow) ?? "-")
                .foregroundStyle(.secondary)
        }
        .width(min: 86, ideal: 104)
        .alignment(.trailing)
    }

    private var ratioColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("倍率", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.ratioSortValue)) { row in
            ratioCell(row)
        }
        .width(min: 70, ideal: 86)
        .alignment(.trailing)
    }

    private var protocolColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("协议", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.protocolSortValue)) { row in
            Text(protocolTitle(row.model.protocolID))
                .help(row.model.capabilityProbeError ?? "")
        }
        .width(min: 90, ideal: 108)
    }

    @TableColumnBuilder<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>>
    private var capabilityColumns: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        imageColumn
        toolSearchColumn
        webSearchColumn
        functionToolsColumn
        thinkingColumn
    }

    private var imageColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("图片", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.imageSortValue)) { row in
            capabilityCell(title: capabilityTitle(row.model.supportsImage), error: nil)
        }
        .width(min: 62, ideal: 72)
    }

    private var toolSearchColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("Tool Search", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.toolSearchSortValue)) { row in
            capabilityCell(title: capabilityTitle(row.model.supportsToolSearch), error: row.model.capabilityProbeError)
        }
        .width(min: 94, ideal: 106)
    }

    private var webSearchColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("Web Search", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.webSearchSortValue)) { row in
            capabilityCell(title: capabilityTitle(row.model.supportsWebSearch), error: row.model.capabilityProbeError)
        }
        .width(min: 94, ideal: 106)
    }

    private var functionToolsColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("Function Tools", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.functionToolsSortValue)) { row in
            capabilityCell(title: capabilityTitle(row.model.supportsFunctionTools), error: row.model.capabilityProbeError)
        }
        .width(min: 108, ideal: 120)
    }

    private var thinkingColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("Thinking", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.thinkingSortValue)) { row in
            capabilityCell(title: capabilityTitle(row.model.supportsThinking), error: nil)
        }
        .width(min: 80, ideal: 88)
    }

    private func selectionBinding(_ row: ModelBenchmarkTableRow) -> Binding<Bool> {
        Binding(
            get: { model.isSelected(row) },
            set: { model.setSelected(row, isSelected: $0) }
        )
    }

    private func modelCell(_ row: ModelBenchmarkTableRow) -> some View {
        let displayName = row.model.displayName.flatMap { $0 == row.model.id ? nil : $0 }
        var suffixes: [String] = []
        if row.model.isNew { suffixes.append("新增") }
        if !row.model.isAvailable { suffixes.append("不可用") }
        let suffix = suffixes.isEmpty ? "" : " · \(suffixes.joined(separator: " / "))"
        return Text((displayName.map { "\(row.model.id) · \($0)" } ?? row.model.id) + suffix)
            .font(.callout.monospaced())
            .lineLimit(1)
            .truncationMode(.middle)
            .foregroundStyle(row.model.isAvailable ? .primary : .secondary)
            .help(row.model.description ?? row.model.id)
    }

    private func latencyCell(_ row: ModelBenchmarkTableRow) -> some View {
        Group {
            if let result = row.result, result.status != "completed" {
                Text(resultStatusTitle(result.status) + (result.ttftMs.map { " · \(formatMilliseconds($0))" } ?? ""))
                    .foregroundStyle(resultStatusColor(result.status))
            } else if let ttft = row.result?.ttftMs {
                Text(formatMilliseconds(ttft))
                    .foregroundStyle(latencyColor(ttft))
            } else {
                Text("-").foregroundStyle(.secondary)
            }
        }
        .help(row.result?.error ?? "")
    }

    private func throughputCell(_ row: ModelBenchmarkTableRow) -> some View {
        let tps = row.result?.tps
        let title = tps.map { String(format: "%.1f tok/s", $0) } ?? "-"
        return Text(title)
            .foregroundStyle(tps == nil ? Color.secondary : Color.primary)
    }

    private func ratioCell(_ row: ModelBenchmarkTableRow) -> some View {
        Text(row.model.ratio ?? "-")
            .help(row.model.priceType ?? "")
    }

    private func capabilityCell(title: String, error: String?) -> some View {
        Text(title)
            .help(error ?? "")
    }

    private var statusBar: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 5) {
                Text(model.summary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(model.statusTitle)
                    .font(.callout.weight(.medium))
                    .foregroundStyle(model.statusColor)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer()
            if model.determinateProgress != nil {
                ProgressView(value: model.determinateProgress ?? 0)
                    .progressViewStyle(.linear)
                    .frame(width: 220)
            } else {
                ProgressView()
                    .controlSize(.small)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }
}

final class ModelBenchmarkWindowController: NSWindowController, NSWindowDelegate {
    private let model: ModelBenchmarkModel

    init(
        startHandler: @escaping (Int, String, Int) async throws -> ModelBenchmarkSnapshot,
        fetchHandler: @escaping () async throws -> ModelBenchmarkSnapshot?,
        loadProvidersHandler: @escaping () async throws -> ProviderListResponse,
        saveSelectionsHandler: @escaping ([String: [String]], OperationProgress) async throws -> Void,
        discoverHandler: @escaping (String, @escaping (String) -> Void) async throws -> Void,
        probeHandler: @escaping (String, @escaping (String) -> Void) async throws -> Void
    ) {
        model = ModelBenchmarkModel(
            startHandler: startHandler,
            fetchHandler: fetchHandler,
            loadProvidersHandler: loadProvidersHandler,
            saveSelectionsHandler: saveSelectionsHandler,
            discoverHandler: discoverHandler,
            probeHandler: probeHandler
        )
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
        window.toolbarStyle = .unified
        if #available(macOS 26.0, *) {
            window.titlebarAppearsTransparent = true
        }
        window.center()
        super.init(window: window)
        window.delegate = self
        installContent()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        model.resetForPresentation()
    }

    func windowWillClose(_ notification: Notification) {
        model.stopPolling()
    }

    private func installContent() {
        let rootView = ModelBenchmarkRootView(model: model)
        let hostingController = NSHostingController(rootView: rootView)
        if #available(macOS 14.0, *) {
            hostingController.sceneBridgingOptions = [.toolbars]
        }
        window?.contentViewController = hostingController
    }
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

private func formatMilliseconds(_ milliseconds: UInt64) -> String {
    if milliseconds < 1_000 { return "\(milliseconds) ms" }
    return String(format: "%.2f s", Double(milliseconds) / 1_000)
}

private func latencyColor(_ milliseconds: UInt64) -> Color {
    switch milliseconds {
    case ..<1_000: return .green
    case ..<3_000: return .orange
    default: return .red
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

private func resultStatusColor(_ status: String) -> Color {
    switch status {
    case "completed": return .green
    case "timed_out": return .orange
    case "failed": return .red
    default: return .secondary
    }
}

private func formatBenchmarkDate(_ milliseconds: UInt64) -> String {
    let date = Date(timeIntervalSince1970: TimeInterval(milliseconds) / 1_000)
    let formatter = DateFormatter()
    formatter.dateStyle = .medium
    formatter.timeStyle = .short
    return formatter.string(from: date)
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
