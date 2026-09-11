import Cocoa
import SwiftUI

private struct NativeSearchField: NSViewRepresentable {
    @Binding var text: String
    let placeholder: String

    func makeCoordinator() -> Coordinator {
        Coordinator(text: $text)
    }

    func makeNSView(context: Context) -> NSSearchField {
        let searchField = NSSearchField()
        searchField.placeholderString = placeholder
        searchField.isBezeled = true
        searchField.bezelStyle = .roundedBezel
        searchField.focusRingType = .default
        searchField.sendsSearchStringImmediately = true
        searchField.target = context.coordinator
        searchField.action = #selector(Coordinator.searchFieldChanged(_:))
        return searchField
    }

    func updateNSView(_ searchField: NSSearchField, context: Context) {
        context.coordinator.text = $text
        searchField.placeholderString = placeholder
        if searchField.stringValue != text {
            searchField.stringValue = text
        }
    }

    final class Coordinator: NSObject {
        var text: Binding<String>

        init(text: Binding<String>) {
            self.text = text
        }

        @objc func searchFieldChanged(_ searchField: NSSearchField) {
            text.wrappedValue = searchField.stringValue
        }
    }
}

struct ModelBenchmarkTableRow: Identifiable {
    let providerID: String
    let model: ProviderModelListItem
    let result: ModelBenchmarkResult?
    let isSelectedSortValue: Int

    var id: String {
        providerModelSelectionKey(providerID: providerID, modelID: model.id)
    }

    var ttftSortValue: UInt64? { result?.ttftMs }
    var tpsSortValue: Double? { result?.tps }
    var contextSortValue: UInt64? { model.contextWindow }
    var ratioSortValue: Double? { benchmarkRatioValue(model.ratio) }
    var capabilitySortValue: Int {
        [
            model.supportsImage,
            model.supportsThinking,
            model.supportsFunctionTools,
            model.supportsToolSearch,
            model.supportsWebSearch,
        ].reduce(0) { score, capability in
            score + (capability == true ? 1 : 0)
        }
    }
}

struct ProviderModelSelectionUpdate {
    let providerID: String
    let modelIDs: [String]
    let modelContexts: [String: UInt64]
}

@MainActor
final class ModelBenchmarkModel: ObservableObject {
    let startHandler: (Int, String, Int) async throws -> ModelBenchmarkSnapshot
    let fetchHandler: () async throws -> ModelBenchmarkSnapshot?
    let loadProvidersHandler: () async throws -> ProviderListResponse
    let saveSelectionsHandler: (
        [String: [String]], [String: [String: UInt64]], OperationProgress
    ) async throws -> Void

    @Published var providers: [ProviderView] = []
    @Published var selectedProviderID: String?
    @Published var visibleRows: [ModelBenchmarkTableRow] = []
    @Published var selectedModelKeys: Set<String> = []
    @Published var savedModelKeys: Set<String> = []
    @Published var query = ""
    @Published var manualModelID = ""
    @Published var selectionFilter = "all"
    @Published var modeSelection = 1
    @Published var timeoutSeconds = 5
    @Published var statusTitle = "请选择服务商"
    @Published var statusColor = Color(nsColor: .secondaryLabelColor)
    @Published var summary = "默认只测试首 token 延迟（TTFT）"
    @Published var determinateProgress: Double?
    @Published var snapshot: ModelBenchmarkSnapshot?
    @Published var isSavingSelections = false
    @Published var isLaunchingBenchmark = false
    @Published var selectionConflictProviderIDs: Set<String> = []

    private(set) var resultCache: [String: ModelBenchmarkResult] = [:]
    private(set) var manualModelIDs: [String: Set<String>] = [:]
    private(set) var manualModelContexts: [String: [String: UInt64]] = [:]
    private(set) var savedManualModelContexts: [String: [String: UInt64]] = [:]
    private(set) var removedManualModelIDs: [String: Set<String>] = [:]
    private var pollingTask: Task<Void, Never>?
    var providerListDidLoad: ((ProviderListResponse, String?) -> Void)?

    init(
        startHandler: @escaping (Int, String, Int) async throws -> ModelBenchmarkSnapshot,
        fetchHandler: @escaping () async throws -> ModelBenchmarkSnapshot?,
        loadProvidersHandler: @escaping () async throws -> ProviderListResponse,
        saveSelectionsHandler: @escaping (
            [String: [String]], [String: [String: UInt64]], OperationProgress
        ) async throws -> Void
    ) {
        self.startHandler = startHandler
        self.fetchHandler = fetchHandler
        self.loadProvidersHandler = loadProvidersHandler
        self.saveSelectionsHandler = saveSelectionsHandler
    }

    var selectedProvider: ProviderView? {
        providers.first { $0.id == selectedProviderID }
    }

    var providerOptions: [ProviderPickerOption] {
        modelSelectionProviderOptions(providers)
    }

    var dirty: Bool {
        selectedModelKeys != savedModelKeys || manualModelContexts != savedManualModelContexts
    }

    var selectedProviderDirty: Bool {
        guard let selectedProviderID else { return false }
        return hasSelectionChanges(for: selectedProviderID)
    }

    var selectedProviderHasExternalChange: Bool {
        selectedProviderID.map(selectionConflictProviderIDs.contains) == true
    }

    var isBusy: Bool {
        isSavingSelections || isLaunchingBenchmark || snapshot?.status == "running"
    }

    var selectedVisibleCount: Int {
        visibleRows.filter { selectedModelKeys.contains($0.id) && $0.model.isAvailable }.count
    }

    var appliedModelCount: Int {
        selectedProvider?.selectedModels.count ?? 0
    }

    var benchmarkActionTitle: String {
        "测试已加入的 \(appliedModelCount) 个模型"
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

    func refreshFromGatewayForPresentation() async {
        stopPolling()
        await refreshFromGateway()
        if snapshot?.status == "running" {
            beginPolling()
        }
    }

    func selectProvider(_ providerID: String?) {
        selectedProviderID = providerID
        rebuildRows()
    }

    func isSelected(_ row: ModelBenchmarkTableRow) -> Bool {
        selectedModelKeys.contains(row.id)
    }

    func setSelected(_ row: ModelBenchmarkTableRow, isSelected: Bool) {
        guard !isBusy, row.model.isAvailable || !isSelected else { return }
        if isSelected {
            selectedModelKeys.insert(row.id)
        } else {
            selectedModelKeys.remove(row.id)
            if row.model.manuallyAdded {
                manualModelContexts[row.providerID]?.removeValue(forKey: row.model.id)
                if manualModelContexts[row.providerID]?.isEmpty == true {
                    manualModelContexts.removeValue(forKey: row.providerID)
                }
            }
        }
    }

    func selectAllVisible() {
        selectedModelKeys.formUnion(visibleRows.filter(\.model.isAvailable).map(\.id))
    }

    func selectNoneVisible() {
        selectedModelKeys.subtract(visibleRows.map(\.id))
    }

    func addManualModel() {
        guard let provider = selectedProvider, provider.kind == .configured, !isBusy else { return }
        let modelID = manualModelID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !modelID.isEmpty else { return }
        let key = providerModelSelectionKey(providerID: provider.id, modelID: modelID)
        removedManualModelIDs[provider.id]?.remove(modelID)
        if !provider.modelItems.contains(where: { $0.id == modelID }) {
            manualModelIDs[provider.id, default: []].insert(modelID)
            manualModelContexts[provider.id, default: [:]][modelID] = 128_000
        }
        selectedModelKeys.insert(key)
        manualModelID = ""
        rebuildRows()
    }

    func removeManualModel(_ row: ModelBenchmarkTableRow) {
        guard row.model.manuallyAdded, !isBusy else { return }
        manualModelIDs[row.providerID]?.remove(row.model.id)
        manualModelContexts[row.providerID]?.removeValue(forKey: row.model.id)
        if manualModelContexts[row.providerID]?.isEmpty == true {
            manualModelContexts.removeValue(forKey: row.providerID)
        }
        removedManualModelIDs[row.providerID, default: []].insert(row.model.id)
        selectedModelKeys.remove(row.id)
        rebuildRows()
    }

    func manualModelContextK(_ row: ModelBenchmarkTableRow) -> UInt64 {
        modelContextK(
            fromTokens: manualModelContexts[row.providerID]?[row.model.id]
                ?? row.model.contextWindow
                ?? 128_000
        )
    }

    func setManualModelContextK(_ row: ModelBenchmarkTableRow, contextK: UInt64) {
        guard row.model.manuallyAdded, !isBusy else { return }
        manualModelContexts[row.providerID, default: [:]][row.model.id] = modelContextTokens(
            fromK: contextK
        )
        rebuildRows()
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

    func selectionUpdate(for providerID: String) -> ProviderModelSelectionUpdate? {
        guard hasSelectionChanges(for: providerID),
              let provider = providers.first(where: { $0.id == providerID })
        else {
            return nil
        }
        let selections = providerModelSelections(
            [provider],
            selectedKeys: selectedModelKeys,
            additionalModelIDs: manualModelIDs
        )
        let selectedContexts = (manualModelContexts[providerID] ?? [:]).filter { modelID, _ in
            selectedModelKeys.contains(
                providerModelSelectionKey(providerID: providerID, modelID: modelID)
            )
        }
        return ProviderModelSelectionUpdate(
            providerID: providerID,
            modelIDs: selections[providerID] ?? [],
            modelContexts: selectedContexts
        )
    }

    func selectionChangeCounts(for providerID: String) -> (added: Int, removed: Int) {
        let prefix = "\(providerID)\u{1f}"
        let selected = Set(selectedModelKeys.filter { $0.hasPrefix(prefix) })
        let saved = Set(savedModelKeys.filter { $0.hasPrefix(prefix) })
        return (selected.subtracting(saved).count, saved.subtracting(selected).count)
    }

    func markSelectionSaved(for providerID: String) {
        let prefix = "\(providerID)\u{1f}"
        savedModelKeys = Set(savedModelKeys.filter { !$0.hasPrefix(prefix) })
            .union(Set(selectedModelKeys.filter { $0.hasPrefix(prefix) }))
        if let contexts = manualModelContexts[providerID], !contexts.isEmpty {
            savedManualModelContexts[providerID] = contexts
        } else {
            savedManualModelContexts.removeValue(forKey: providerID)
        }
        selectionConflictProviderIDs.remove(providerID)
    }

    func discardSelectionDrafts() {
        selectedModelKeys = savedModelKeys
        manualModelIDs.removeAll()
        removedManualModelIDs.removeAll()
        manualModelContexts = savedManualModelContexts
        selectionConflictProviderIDs.removeAll()
        rebuildRows()
    }

    func discardSelectionDraft(for providerID: String) {
        let prefix = "\(providerID)\u{1f}"
        selectedModelKeys = Set(selectedModelKeys.filter { !$0.hasPrefix(prefix) })
            .union(Set(savedModelKeys.filter { $0.hasPrefix(prefix) }))
        manualModelIDs.removeValue(forKey: providerID)
        removedManualModelIDs.removeValue(forKey: providerID)
        if let contexts = savedManualModelContexts[providerID] {
            manualModelContexts[providerID] = contexts
        } else {
            manualModelContexts.removeValue(forKey: providerID)
        }
        selectionConflictProviderIDs.remove(providerID)
        rebuildRows()
    }

    func acknowledgeSelectedExternalChange() {
        guard let selectedProviderID else { return }
        selectionConflictProviderIDs.remove(selectedProviderID)
    }

    func startBenchmark() {
        guard let providerID = selectedProviderID, !isBusy, !selectedProviderDirty,
              appliedModelCount > 0
        else { return }
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

    func reloadProviders(selecting providerID: String? = nil) {
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await loadProviders(selecting: providerID)
            } catch {
                handleProviderLoadFailure(error)
            }
        }
    }

    func applyProviderList(_ response: ProviderListResponse, selecting providerID: String?) {
        let previousSelected = selectedModelKeys
        let previousSaved = savedModelKeys
        let changedKeys = previousSelected.union(previousSaved).filter {
            previousSelected.contains($0) != previousSaved.contains($0)
        }
        providers = response.providers
        reconcileManualModelState()
        selectedProviderID = providerID ?? selectedProviderID
            ?? providers.first(where: { $0.kind == .configured })?.id
        let loadedSaved = selectedProviderModelKeys(providers)
        for provider in providers where hasSelectionChanges(for: provider.id) {
            let prefix = "\(provider.id)\u{1f}"
            let previousProviderSaved = Set(previousSaved.filter { $0.hasPrefix(prefix) })
            let loadedProviderSaved = Set(loadedSaved.filter { $0.hasPrefix(prefix) })
            if previousProviderSaved != loadedProviderSaved {
                selectionConflictProviderIDs.insert(provider.id)
            }
        }
        savedModelKeys = loadedSaved
        selectedModelKeys = loadedSaved
        for key in changedKeys {
            if previousSelected.contains(key) {
                selectedModelKeys.insert(key)
            } else {
                selectedModelKeys.remove(key)
            }
        }
        rebuildRows()
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
        let modelItems = mergedProviderModelItems(
            provider,
            additionalModelIDs: manualModelIDs[provider.id] ?? [],
            excludingModelIDs: removedManualModelIDs[provider.id] ?? []
        )
        let rows = modelItems.compactMap { model -> ModelBenchmarkTableRow? in
            let key = providerModelSelectionKey(providerID: provider.id, modelID: model.id)
            let matchesQuery = searchText.isEmpty
                || model.id.localizedCaseInsensitiveContains(searchText)
                || model.displayName?.localizedCaseInsensitiveContains(searchText) == true
                || model.description?.localizedCaseInsensitiveContains(searchText) == true
            let matchesFilter: Bool
            switch filter {
            case "selected": matchesFilter = selectedModelKeys.contains(key)
            case "new": matchesFilter = model.isNew
            case "attention":
                matchesFilter = !model.isAvailable
                    || resultCache[key].map { $0.status == "failed" || $0.status == "timed_out" }
                        == true
            default: matchesFilter = true
            }
            guard matchesQuery && matchesFilter else { return nil }
            return ModelBenchmarkTableRow(
                providerID: provider.id,
                model: model,
                result: resultCache[key],
                isSelectedSortValue: selectedModelKeys.contains(key) ? 1 : 0
            )
        }
        visibleRows = rows.sorted(using: sortOverride ?? [
            KeyPathComparator(\ModelBenchmarkTableRow.model.id, comparator: .localizedStandard),
        ])
    }

    private func reconcileManualModelState() {
        for provider in providers {
            let cachedModelIDs = Set(provider.modelItems.map(\.id))
            manualModelIDs[provider.id]?.subtract(cachedModelIDs)
            if manualModelIDs[provider.id]?.isEmpty == true {
                manualModelIDs.removeValue(forKey: provider.id)
            }
            let cachedManualIDs = Set(
                provider.modelItems.filter(\.manuallyAdded).map(\.id)
            )
            removedManualModelIDs[provider.id]?.formIntersection(cachedManualIDs)
            if removedManualModelIDs[provider.id]?.isEmpty == true {
                removedManualModelIDs.removeValue(forKey: provider.id)
            }
        }
    }

    private func hasSelectionChanges(for providerID: String) -> Bool {
        let prefix = "\(providerID)\u{1f}"
        let selected = Set(selectedModelKeys.filter { $0.hasPrefix(prefix) })
        let saved = Set(savedModelKeys.filter { $0.hasPrefix(prefix) })
        return selected != saved
            || manualModelContexts[providerID] != savedManualModelContexts[providerID]
    }

    private func loadProviders(selecting providerID: String?) async throws {
        let response = try await loadProvidersHandler()
        applyProviderList(response, selecting: providerID)
        providerListDidLoad?(response, selectedProviderID)
    }

    private func handleProviderLoadFailure(_ error: Error) {
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

    private func persistSelectionsIfNeeded() async throws {
        guard dirty else { return }
        isSavingSelections = true
        defer {
            isSavingSelections = false
        }
        setStatus("正在保存模型选择并更新 Codex 目录…", color: .secondary)
        let selections = providerModelSelections(
            providers,
            selectedKeys: selectedModelKeys,
            additionalModelIDs: manualModelIDs
        )
        let selectedContexts = Dictionary(uniqueKeysWithValues: manualModelContexts.compactMap {
            providerID, contexts in
            let selected = contexts.filter { modelID, _ in
                selectedModelKeys.contains(
                    providerModelSelectionKey(providerID: providerID, modelID: modelID)
                )
            }
            return selected.isEmpty ? nil : (providerID, selected)
        })
        try await runOperationProgress(
            title: "正在保存模型选择",
            phases: ["写入模型选择", "重启本地网关", "刷新 Codex 模型目录", "完成"],
            successTitle: "✓ 模型选择已保存",
            failureTitle: "✗ 保存失败",
            showFailureAlert: false
        ) { progress in
            try await self.saveSelectionsHandler(selections, selectedContexts, progress)
        }
        savedModelKeys = selectedModelKeys
        savedManualModelContexts = manualModelContexts
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
                setStatus("当前服务商尚未测速", color: .secondary)
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

    private func setStatus(_ title: String, color: Color) {
        statusTitle = title
        statusColor = color
    }

}

struct ModelBenchmarkRootView: View {
    @ObservedObject var model: ModelBenchmarkModel
    var embedded = false
    var onApplyChanges: (() -> Void)? = nil
    var canApplyChanges = false
    var applySummary: String? = nil
    var hasExternalDraft = false
    var isExternallyBusy = false
    @State private var showsManualModelEntry = false
    @State private var sortOrder: [KeyPathComparator<ModelBenchmarkTableRow>] = [
        KeyPathComparator(\ModelBenchmarkTableRow.model.id, comparator: .localizedStandard),
    ]

    var body: some View {
        VStack(spacing: 0) {
            benchmarkToolbar
            selectionToolbar

            modelTableArea
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(nsColor: .textBackgroundColor))
            .overlay {
                Rectangle()
                    .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5)
            }

            Divider()
            statusBar
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .onAppear {
            model.rebuildRows(sortOverride: sortOrder)
        }
        .onChange(of: sortOrder) { newValue in
            model.rebuildRows(sortOverride: newValue)
        }
        .onChange(of: model.query) { _ in
            model.rebuildRows(sortOverride: sortOrder)
        }
        .onChange(of: model.selectionFilter) { _ in
            model.rebuildRows(sortOverride: sortOrder)
        }
        .animation(.easeInOut(duration: 0.18), value: showsManualModelEntry)
    }

    @ViewBuilder
    private var modelTableArea: some View {
        if model.visibleRows.isEmpty {
            modelTableEmptyState
        } else {
            GeometryReader { geometry in
                ScrollView(.horizontal, showsIndicators: true) {
                    modelTable
                        .frame(
                            width: max(geometry.size.width, minimumModelTableWidth),
                            height: geometry.size.height
                        )
                }
            }
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
            ? "没有可用的服务商"
            : "当前服务商没有模型"
    }

    private var minimumModelTableWidth: CGFloat {
        modelTableMinimumWidth(
            includesRatio: model.selectedProvider?.presetID == "baidu-oneapi"
        )
    }

    @ViewBuilder
    private var modelTable: some View {
        if model.selectedProvider?.presetID == "baidu-oneapi" {
            Table(model.visibleRows, sortOrder: $sortOrder) {
                selectedColumn
                modelColumn
                latencyColumn
                throughputColumn
                contextColumn
                ratioColumn
                capabilityColumns
            }
            .scrollContentBackground(.hidden)
        } else {
            Table(model.visibleRows, sortOrder: $sortOrder) {
                selectedColumn
                modelColumn
                latencyColumn
                throughputColumn
                contextColumn
                capabilityColumns
            }
            .scrollContentBackground(.hidden)
        }
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
        HStack(spacing: 8) {
            if !embedded {
                Picker("服务商", selection: providerBinding) {
                    ForEach(model.providerOptions, id: \.id) { option in
                        Text(option.displayName).tag(Optional(option.id))
                    }
                }
                .labelsHidden()
                .frame(width: 190)
            }

            NativeSearchField(
                text: searchBinding,
                placeholder: "搜索当前服务商的模型"
            )
            .frame(minWidth: 220, idealWidth: 320, maxWidth: 420)
            .layoutPriority(1)
            .accessibilityLabel("搜索当前服务商的模型")

            Menu {
                Picker("筛选", selection: filterBinding) {
                    Text("全部模型").tag("all")
                    Text("已加入 Codex").tag("selected")
                    Text("新增").tag("new")
                    Text("需要处理").tag("attention")
                }
            } label: {
                Label(filterTitle, systemImage: "line.3.horizontal.decrease")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()

            modelActionsMenu

            Spacer()

            primaryAction
        }
        .padding(.horizontal, 20)
        .padding(.top, 14)
        .padding(.bottom, 12)
    }

    @ViewBuilder
    private var selectionToolbar: some View {
        if showsManualModelEntry {
            HStack(spacing: 8) {
                TextField("手动输入模型 ID（可不在 /v1/models 中）", text: $model.manualModelID)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit(model.addManualModel)
                Button("添加并选中", action: model.addManualModel)
                    .disabled(
                        model.isBusy || isExternallyBusy
                            || model.selectedProvider?.kind != .configured
                            || model.manualModelID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    )
                Text("能力将由后台自动补齐")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button {
                    showsManualModelEntry = false
                } label: {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.borderless)
                .help("收起手动添加")
            }
            .padding(.horizontal, 20)
            .padding(.bottom, 12)
            .transition(.move(edge: .top).combined(with: .opacity))
        }
    }

    private var filterTitle: String {
        switch model.selectionFilter {
        case "selected": return "已加入"
        case "new": return "新增"
        case "attention": return "需处理"
        default: return "全部"
        }
    }

    private var modelActionsMenu: some View {
        Menu {
            Picker("测速内容", selection: modeBinding) {
                Text("仅测试延迟（TTFT）").tag(1)
                Text("测试延迟与吞吐").tag(100)
            }

            Picker("请求超时", selection: timeoutBinding) {
                ForEach([5, 10, 20, 30, 60], id: \.self) { seconds in
                    Text("\(seconds) 秒").tag(seconds)
                }
            }

            Divider()

            Button("全选当前结果", action: model.selectAllVisible)
                .disabled(model.isBusy || isExternallyBusy || model.visibleRows.isEmpty)
            Button("取消选择当前结果", action: model.selectNoneVisible)
                .disabled(model.isBusy || isExternallyBusy || model.visibleRows.isEmpty)
            Button("手动添加模型…") {
                showsManualModelEntry = true
            }
            .disabled(model.isBusy || isExternallyBusy || model.selectedProvider?.kind != .configured)
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .menuStyle(.borderlessButton)
        .help(benchmarkConfigurationSummary)
        .accessibilityLabel("更多模型操作")
    }

    @ViewBuilder
    private var primaryAction: some View {
        if hasPendingChanges, let onApplyChanges {
            Button(action: onApplyChanges) {
                Label("应用更改", systemImage: "square.and.arrow.down")
            }
            .keyboardShortcut("s", modifiers: .command)
            .liquidGlassProminentButton()
            .disabled(!canApplyChanges)
            .help(applySummary ?? "应用当前服务商的更改")
        } else if onApplyChanges == nil, model.dirty {
            Button {
                model.saveSelections()
            } label: {
                Label("保存模型选择", systemImage: "square.and.arrow.down")
            }
            .liquidGlassProminentButton()
            .disabled(model.isBusy)
        } else {
            Button(action: model.startBenchmark) {
                Label(model.benchmarkActionTitle, systemImage: "speedometer")
            }
            .keyboardShortcut(.defaultAction)
            .liquidGlassProminentButton()
            .disabled(
                model.isBusy || isExternallyBusy || model.appliedModelCount == 0
                    || model.selectedProvider?.kind != .configured
            )
            .help(benchmarkConfigurationSummary)
        }
    }

    private var hasPendingChanges: Bool {
        hasExternalDraft || model.selectedProviderDirty
    }

    private var benchmarkConfigurationSummary: String {
        let mode = model.modeSelection == 1 ? "仅测试延迟（TTFT）" : "测试延迟与吞吐"
        return "\(mode)，请求超时 \(model.timeoutSeconds) 秒"
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
        TableColumn("Codex", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.isSelectedSortValue)) { row in
            Toggle("", isOn: selectionBinding(row))
                .labelsHidden()
                .disabled(
                    (!row.model.isAvailable && !model.isSelected(row))
                        || model.isBusy || isExternallyBusy
                )
        }
        .width(min: 56, ideal: 64)
    }

    private var modelColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn(
            "模型",
            sortUsing: KeyPathComparator(
                \ModelBenchmarkTableRow.model.id,
                comparator: .localizedStandard
            )
        ) { row in
            modelCell(row)
        }
        .width(min: 260, ideal: 520)
    }

    private var latencyColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("首 Token", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.ttftSortValue)) { row in
            latencyCell(row)
                .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .width(min: 84, ideal: 104)
    }

    private var throughputColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("生成速度", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.tpsSortValue)) { row in
            throughputCell(row)
                .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .width(min: 92, ideal: 112)
    }

    private var contextColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("上下文", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.contextSortValue)) { row in
            if row.model.manuallyAdded {
                HStack(spacing: 2) {
                    TextField("", value: manualContextBinding(row), format: .number)
                        .textFieldStyle(.plain)
                        .multilineTextAlignment(.trailing)
                    Text("K")
                        .foregroundStyle(.secondary)
                }
                .disabled(model.isBusy || isExternallyBusy)
                .help("手动模型上下文，单位 K；修改后保存模型选择")
            } else {
                Text(row.model.contextWindow.map(formatContextWindow) ?? "-")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
        }
        .width(min: 86, ideal: 104)
    }

    private var ratioColumn: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn("倍率", sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.ratioSortValue)) { row in
            ratioCell(row)
                .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .width(min: 70, ideal: 86)
    }

    private var capabilityColumns: some TableColumnContent<ModelBenchmarkTableRow, KeyPathComparator<ModelBenchmarkTableRow>> {
        TableColumn(
            "能力",
            sortUsing: KeyPathComparator(\ModelBenchmarkTableRow.capabilitySortValue)
        ) { row in
            capabilityCell(row)
        }
        .width(min: 150, ideal: 170)
    }

    private func selectionBinding(_ row: ModelBenchmarkTableRow) -> Binding<Bool> {
        Binding(
            get: { model.isSelected(row) },
            set: { model.setSelected(row, isSelected: $0) }
        )
    }

    private func manualContextBinding(_ row: ModelBenchmarkTableRow) -> Binding<UInt64> {
        Binding(
            get: { model.manualModelContextK(row) },
            set: { model.setManualModelContextK(row, contextK: $0) }
        )
    }

    private func modelCell(_ row: ModelBenchmarkTableRow) -> some View {
        let displayName = row.model.displayName.flatMap { $0 == row.model.id ? nil : $0 }
        var suffixes: [String] = []
        if row.model.isNew { suffixes.append("新增") }
        if !row.model.isAvailable { suffixes.append("不可用") }
        let isSaved = model.savedModelKeys.contains(row.id)
        let isSelected = model.selectedModelKeys.contains(row.id)
        if isSaved != isSelected {
            suffixes.append(isSelected ? "待加入" : "待移除")
        }
        let suffix = suffixes.isEmpty ? "" : " · \(suffixes.joined(separator: " / "))"
        return HStack(spacing: 6) {
            Text((displayName.map { "\(row.model.id) · \($0)" } ?? row.model.id) + suffix)
                .font(.callout.monospaced())
                .lineLimit(1)
                .truncationMode(.middle)
                .foregroundStyle(row.model.isAvailable ? .primary : .secondary)
                .help(row.model.description ?? row.model.id)
            Spacer(minLength: 4)
            if row.model.manuallyAdded {
                Button {
                    model.removeManualModel(row)
                } label: {
                    Image(systemName: "trash")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.borderless)
                .disabled(model.isBusy || isExternallyBusy)
                .help("删除手动模型；保存模型选择后生效")
            }
        }
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

    private func capabilityCell(_ row: ModelBenchmarkTableRow) -> some View {
        HStack(spacing: 8) {
            capabilityIcon(
                title: "图片输入",
                systemImage: "photo",
                supported: row.model.supportsImage,
                error: row.model.capabilityProbeError
            )
            capabilityIcon(
                title: "Thinking",
                systemImage: "brain.head.profile",
                supported: row.model.supportsThinking,
                error: row.model.capabilityProbeError
            )
            capabilityIcon(
                title: "Function Tools",
                systemImage: "hammer",
                supported: row.model.supportsFunctionTools,
                error: row.model.capabilityProbeError
            )
            capabilityIcon(
                title: "Tool Search",
                systemImage: "magnifyingglass",
                supported: row.model.supportsToolSearch,
                error: row.model.capabilityProbeError
            )
            capabilityIcon(
                title: "Web Search",
                systemImage: "globe",
                supported: row.model.supportsWebSearch,
                error: row.model.capabilityProbeError
            )
        }
    }

    private func capabilityIcon(
        title: String,
        systemImage: String,
        supported: Bool?,
        error: String?
    ) -> some View {
        let description = capabilityDescription(title: title, supported: supported, error: error)
        return Image(systemName: systemImage)
            .frame(width: 18)
            .foregroundStyle(capabilityColor(supported))
            .opacity(supported == false ? 0.55 : 1)
            .help(description)
            .accessibilityLabel(description)
    }

    private var statusBar: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 5) {
                Text(model.summary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(pendingChangesStatus ?? model.statusTitle)
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
            } else if model.isBusy {
                ProgressView()
                    .controlSize(.small)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
        .background(Color(nsColor: .controlBackgroundColor))
    }

    private var pendingChangesStatus: String? {
        if isExternallyBusy {
            return "正在更新当前服务商配置…"
        }
        guard hasExternalDraft || model.selectedProviderDirty else { return nil }
        return "先应用更改，再测试。测速不会自动保存选择。"
    }
}

private func compareOptionalNumbers<T: BinaryInteger>(
    _ left: T?,
    _ right: T?
) -> ComparisonResult {
    compareOptionalNumbers(left.map(Double.init), right.map(Double.init))
}

private func capabilityDescription(title: String, supported: Bool?, error: String?) -> String {
    let status: String
    switch supported {
    case true: status = "支持"
    case false: status = "不支持"
    case nil: status = "未知"
    }
    guard let error, !error.isEmpty else { return "\(title)：\(status)" }
    return "\(title)：\(status)。能力探测：\(error)"
}

private func capabilityColor(_ supported: Bool?) -> Color {
    switch supported {
    case true: return .accentColor
    case false: return Color(nsColor: .tertiaryLabelColor)
    case nil: return .orange
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
    presentPersistentWindow(alert.window)
    alert.runModal()
    alert.window.close()
}

private func presentBenchmarkMessage(title: String, message: String) {
    let alert = NSAlert()
    alert.messageText = localizedPrompt(title)
    alert.informativeText = localizedGatewayMessage(message)
    alert.alertStyle = .informational
    alert.addButton(withTitle: AppLocalization.string("modelBenchmark.ok2"))
    presentPersistentWindow(alert.window)
    alert.runModal()
    alert.window.close()
}
