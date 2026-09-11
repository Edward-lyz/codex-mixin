import Cocoa
import SwiftUI

@MainActor
final class FusionSettingsModel: ObservableObject {
    typealias LoadHandler = () async throws -> FusionSettingsProfile
    typealias FetchModelsHandler = () async throws -> [FusionModelOption]
    typealias SaveHandler = (FusionSettingsProfile, String, OperationProgress) async throws -> Void
    typealias DeleteHandler = (String, OperationProgress) async throws -> Void

    private let loadHandler: LoadHandler
    private let fetchModelsHandler: FetchModelsHandler
    private let saveHandler: SaveHandler
    private let deleteHandler: DeleteHandler
    private var loadedProfile = FusionSettingsProfile()

    @Published var options: [FusionModelOption] = []
    @Published var selectedPanels: Set<String> = []
    @Published var profileID = "default"
    @Published var mode = FusionSettingsMode.orchestration
    @Published var timeRoutes: [FusionTimeRoute] = []
    @Published var judgeModel = ""
    @Published var finalModel = ""
    @Published var minSuccessful = "1"
    @Published var timeoutMs = "300000"
    @Published var showIntermediateResults = false
    @Published var panelToolsEnabled = false
    @Published var status = "正在读取配置..."
    @Published var statusColor = Color.secondary
    @Published var isBusy = false

    init(
        loadHandler: @escaping LoadHandler,
        fetchModelsHandler: @escaping FetchModelsHandler,
        saveHandler: @escaping SaveHandler,
        deleteHandler: @escaping DeleteHandler
    ) {
        self.loadHandler = loadHandler
        self.fetchModelsHandler = fetchModelsHandler
        self.saveHandler = saveHandler
        self.deleteHandler = deleteHandler
    }

    var validationError: String? {
        let trimmedProfileID = profileID.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmedProfileID.isEmpty || trimmedProfileID.contains("/") {
            return "Profile ID 不能为空且不能包含 /。"
        }
        if mode == .timeRotation {
            if finalModel.isEmpty { return "请选择未覆盖时段使用的默认模型。" }
            if finalModel.hasPrefix("mixin/fusion/")
                || timeRoutes.contains(where: { $0.model.hasPrefix("mixin/fusion/") })
            {
                return "Fusion profile 不能递归引用 mixin/fusion/ 模型。"
            }
            return fusionTimeRouteValidationError(timeRoutes)
        }
        if !(1...8).contains(selectedPanels.count) {
            return "请选择 1–8 个 Panel 模型。"
        }
        if judgeModel.isEmpty || finalModel.isEmpty {
            return "Judge 和 Final 模型不能为空。"
        }
        if judgeModel.hasPrefix("mixin/fusion/")
            || finalModel.hasPrefix("mixin/fusion/")
            || selectedPanels.contains(where: { $0.hasPrefix("mixin/fusion/") })
        {
            return "Fusion profile 不能递归引用 mixin/fusion/ 模型。"
        }
        guard let minimum = Int(minSuccessful), (1...selectedPanels.count).contains(minimum) else {
            return "min_successful 必须在 1 和 Panel 数量之间。"
        }
        guard let timeout = Int(timeoutMs), timeout > 0 else {
            return "timeout_ms 必须大于 0。"
        }
        return nil
    }

    var canSave: Bool { !isBusy && !options.isEmpty && validationError == nil }
    var canDisable: Bool { !isBusy && hasConfiguredProfile }

    func reload() {
        isBusy = true
        status = "正在从本地网关读取模型列表..."
        statusColor = .secondary
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                loadedProfile = try await loadHandler()
                applyLoadedProfile()
                let fetched = try await fetchModelsHandler()
                var optionsByID = Dictionary(uniqueKeysWithValues: fetched.map { ($0.id, $0) })
                let configuredIDs = loadedProfile.panelModels
                    + [loadedProfile.judgeModel, loadedProfile.finalModel]
                    + loadedProfile.timeRoutes.map(\.model)
                for id in configuredIDs where !id.isEmpty && !id.hasPrefix("mixin/fusion/") {
                    optionsByID[id] = optionsByID[id] ?? FusionModelOption(
                        id: id,
                        displayName: L10n.Fusion.unavailableModel,
                        isAvailable: false
                    )
                }
                options = optionsByID.values.sorted {
                    $0.id.localizedStandardCompare($1.id) == .orderedAscending
                }
                let selection = resolveFusionModelSelection(
                    availableModelIDs: options.map(\.id),
                    storedPanelModels: loadedProfile.panelModels,
                    storedJudgeModel: loadedProfile.judgeModel,
                    storedFinalModel: loadedProfile.finalModel
                )
                loadedProfile.panelModels = selection.panelModels
                loadedProfile.judgeModel = selection.judgeModel
                loadedProfile.finalModel = selection.finalModel
                selectedPanels = Set(selection.panelModels)
                judgeModel = selection.judgeModel
                finalModel = selection.finalModel
                isBusy = false
                refreshValidationStatus(
                    validMessage: "已加载 \(options.count) 个跨 Provider 模型。Panel 可选择 1–8 个。"
                )
            } catch {
                options = []
                selectedPanels = []
                isBusy = false
                status = localizedErrorDescription(error)
                statusColor = .red
            }
        }
    }

    func togglePanel(_ id: String) {
        guard !isBusy else { return }
        if selectedPanels.contains(id) {
            selectedPanels.remove(id)
        } else if selectedPanels.count < 8 {
            selectedPanels.insert(id)
        } else {
            status = "Panel 最多选择 8 个模型。"
            statusColor = .red
            return
        }
        refreshValidationStatus()
    }

    func modeDidChange() {
        if mode == .timeRotation, timeRoutes.isEmpty, let model = defaultScheduledModel {
            timeRoutes = [FusionTimeRoute(
                startMinute: 9 * 60,
                endMinute: 18 * 60,
                model: model
            )]
        }
        refreshValidationStatus()
    }

    func addTimeRoute() {
        guard timeRoutes.count < 24, let model = defaultScheduledModel else { return }
        let startMinute = timeRoutes.last?.endMinute ?? 9 * 60
        let endMinute = (startMinute + 4 * 60) % 1_440
        timeRoutes.append(FusionTimeRoute(
            startMinute: startMinute,
            endMinute: endMinute,
            model: model
        ))
        refreshValidationStatus()
    }

    func removeTimeRoute(_ id: UUID) {
        timeRoutes.removeAll { $0.id == id }
        refreshValidationStatus()
    }

    func refreshValidationStatus(validMessage: String? = nil) {
        if let validationError {
            status = validationError
            statusColor = .red
        } else {
            status = validMessage ?? "配置有效：已选择 \(selectedPanels.count) 个 Panel 模型。"
            statusColor = .secondary
        }
    }

    func save() {
        refreshValidationStatus()
        guard canSave,
              let minimum = Int(minSuccessful),
              let timeout = Int(timeoutMs)
        else { return }

        var profile = loadedProfile
        profile.id = profileID.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.mode = mode
        profile.timeRoutes = timeRoutes
        profile.panelModels = options.map(\.id).filter(selectedPanels.contains)
        profile.judgeModel = judgeModel
        profile.finalModel = finalModel
        profile.minSuccessful = minimum
        profile.timeoutMs = timeout
        profile.showIntermediateResults = showIntermediateResults
        profile.panelToolsEnabled = panelToolsEnabled
        let replacedProfileID = loadedProfile.id

        isBusy = true
        status = "正在保存并重启本地网关..."
        statusColor = .accentColor
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: "正在保存 Fusion 配置",
                    phases: ["写入 Fusion 配置", "重启本地网关", "刷新 Codex 模型目录", "完成"],
                    successTitle: "✓ Fusion 配置已保存",
                    failureTitle: "✗ 保存失败",
                    showFailureAlert: false
                ) { progress in
                    try await self.saveHandler(profile, replacedProfileID, progress)
                }
                loadedProfile = profile
                isBusy = false
                status = L10n.Fusion.saveSuccess(profile.id)
                statusColor = .green
            } catch {
                isBusy = false
                status = L10n.Fusion.saveFailed(localizedErrorDescription(error))
                statusColor = .red
                presentFusionAlert(
                    title: L10n.Fusion.saveAlertTitle,
                    message: localizedErrorDescription(error)
                )
            }
        }
    }

    func disableFusion() {
        let configuredProfileID = loadedProfile.id.trimmingCharacters(in: .whitespacesAndNewlines)
        guard canDisable, confirm(
            title: L10n.Fusion.disableConfirmTitle,
            message: L10n.Fusion.disableConfirmMessage(configuredProfileID)
        ) else { return }

        isBusy = true
        status = "正在关闭 Fusion 并从模型目录移除..."
        statusColor = .accentColor
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: "正在关闭 Fusion",
                    phases: ["删除 Fusion 配置", "重启本地网关", "刷新 Codex 模型目录", "完成"],
                    successTitle: "✓ Fusion 已关闭",
                    failureTitle: "✗ 关闭失败",
                    showFailureAlert: false
                ) { progress in
                    try await self.deleteHandler(configuredProfileID, progress)
                }
                loadedProfile = FusionSettingsProfile()
                applyLoadedProfile()
                let selection = resolveFusionModelSelection(
                    availableModelIDs: options.map(\.id),
                    storedPanelModels: [],
                    storedJudgeModel: "",
                    storedFinalModel: ""
                )
                judgeModel = selection.judgeModel
                finalModel = selection.finalModel
                selectedPanels = []
                isBusy = false
                status = L10n.Fusion.disableSuccess(configuredProfileID)
                statusColor = .green
            } catch {
                isBusy = false
                status = L10n.Fusion.disableFailed(localizedErrorDescription(error))
                statusColor = .red
                presentFusionAlert(
                    title: L10n.Fusion.disableAlertTitle,
                    message: localizedErrorDescription(error)
                )
            }
        }
    }

    private var hasConfiguredProfile: Bool {
        let configuredProfileID = loadedProfile.id.trimmingCharacters(in: .whitespacesAndNewlines)
        return !configuredProfileID.isEmpty
            && !(loadedProfile.panelModels.isEmpty
                && loadedProfile.judgeModel.isEmpty
                && loadedProfile.finalModel.isEmpty)
    }

    private var defaultScheduledModel: String? {
        if !finalModel.isEmpty { return finalModel }
        return options.first(where: \.isAvailable)?.id
    }

    private func applyLoadedProfile() {
        profileID = loadedProfile.id
        mode = loadedProfile.mode
        timeRoutes = loadedProfile.timeRoutes
        selectedPanels = Set(loadedProfile.panelModels)
        judgeModel = loadedProfile.judgeModel
        finalModel = loadedProfile.finalModel
        minSuccessful = String(loadedProfile.minSuccessful)
        timeoutMs = String(loadedProfile.timeoutMs)
        showIntermediateResults = loadedProfile.showIntermediateResults
        panelToolsEnabled = loadedProfile.panelToolsEnabled
    }
}

private struct FusionSettingsView: View {
    @ObservedObject var model: FusionSettingsModel
    let close: () -> Void
    @State private var panelQuery = ""
    @State private var showsAdvancedOptions = false

    var body: some View {
        VStack(spacing: 0) {
            Form {
                profileSection

                if model.mode == .orchestration {
                    panelSection
                    orchestrationSection
                } else {
                    timeRotationSection
                }

                advancedSection
            }
            .formStyle(.grouped)
            .scrollContentBackground(.hidden)
            .frame(maxWidth: 780, maxHeight: .infinity)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .disabled(model.isBusy)
            .onChange(of: model.selectedPanels) { _ in model.refreshValidationStatus() }
            .onChange(of: model.judgeModel) { _ in model.refreshValidationStatus() }
            .onChange(of: model.finalModel) { _ in model.refreshValidationStatus() }
            .onChange(of: model.minSuccessful) { _ in model.refreshValidationStatus() }
            .onChange(of: model.timeoutMs) { _ in model.refreshValidationStatus() }
            .onChange(of: model.mode) { _ in model.modeDidChange() }
            .onChange(of: model.timeRoutes) { _ in model.refreshValidationStatus() }

            Divider()
            HStack(spacing: 10) {
                Image(systemName: model.validationError == nil ? "checkmark.circle" : "exclamationmark.triangle")
                    .foregroundStyle(model.statusColor)
                Text(model.status)
                    .font(.caption)
                    .foregroundStyle(model.statusColor)
                    .lineLimit(1)
                if model.isBusy {
                    ProgressView().controlSize(.small)
                }
                Spacer()
                Menu {
                    Button(L10n.Fusion.disableButton, action: model.disableFusion)
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .menuStyle(.borderlessButton)
                .help("更多 Fusion 操作")
                .accessibilityLabel("更多 Fusion 操作")
                    .disabled(!model.canDisable)
                Button("关闭", action: close)
                    .keyboardShortcut(.cancelAction)
                Button("保存并重启网关", action: model.save)
                    .keyboardShortcut(.defaultAction)
                    .liquidGlassProminentButton()
                    .disabled(!model.canSave)
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 12)
            .background(Color(nsColor: .controlBackgroundColor))
        }
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var profileSection: some View {
        Section {
            TextField("Profile ID", text: $model.profileID)
                .onChange(of: model.profileID) { _ in model.refreshValidationStatus() }
            Picker("运行模式", selection: $model.mode) {
                ForEach(FusionSettingsMode.allCases) { mode in
                    Text(mode.title).tag(mode)
                }
            }
            .pickerStyle(.segmented)
        } header: {
            Text("Fusion 模型编排")
        } footer: {
            Text(modeDescription)
        }
    }

    private var modeDescription: String {
        switch model.mode {
        case .orchestration:
            return "多个 Panel 模型并行分析，由 Judge 结构化对比，再由 Final 模型回答。"
        case .timeRotation:
            return "按 Mac 当前本地时间选择模型；未覆盖的时段使用默认模型。"
        }
    }

    private var panelSection: some View {
        Section {
            TextField("搜索 Panel 模型", text: $panelQuery)
                .textFieldStyle(.roundedBorder)
            FusionPanelList(model: model, query: panelQuery)
                .frame(minHeight: 160, idealHeight: 240, maxHeight: 280)
        } header: {
            Text("Panel 模型")
        } footer: {
            Text("已选择 \(model.selectedPanels.count) / 8 个；Panel 会并行执行。")
        }
    }

    private var orchestrationSection: some View {
        Section("编排模型") {
            Picker("Judge 模型", selection: $model.judgeModel) {
                modelOptions
            }
            Picker("Final 模型", selection: $model.finalModel) {
                modelOptions
            }
        }
    }

    private var timeRotationSection: some View {
        Section {
            Picker("默认模型", selection: $model.finalModel) {
                modelOptions
            }

            ForEach($model.timeRoutes) { $route in
                HStack(spacing: 8) {
                    DatePicker(
                        "开始",
                        selection: timeBinding($route.startMinute),
                        displayedComponents: .hourAndMinute
                    )
                    .labelsHidden()
                    .frame(width: 92)
                    Text("至")
                        .foregroundStyle(.secondary)
                    DatePicker(
                        "结束",
                        selection: timeBinding($route.endMinute),
                        displayedComponents: .hourAndMinute
                    )
                    .labelsHidden()
                    .frame(width: 92)
                    Picker("模型", selection: $route.model) {
                        modelOptions
                    }
                    .labelsHidden()
                    .frame(maxWidth: .infinity)
                    Button {
                        model.removeTimeRoute(route.id)
                    } label: {
                        Image(systemName: "trash")
                    }
                    .buttonStyle(.borderless)
                    .help("删除时段")
                    .accessibilityLabel("删除时段")
                }
            }

            Button {
                model.addTimeRoute()
            } label: {
                Label("添加时段", systemImage: "plus")
            }
            .disabled(model.timeRoutes.count >= 24 || model.options.isEmpty)
        } header: {
            Text("时段路由")
        } footer: {
            Text("开始时间包含、结束时间不包含；结束早于开始表示跨午夜。时段不能重叠。")
        }
    }

    private var advancedSection: some View {
        Section {
            DisclosureGroup("高级选项", isExpanded: $showsAdvancedOptions) {
                if model.mode == .orchestration {
                    TextField("最少成功 Panel", text: $model.minSuccessful)
                    TextField("单模型超时 (ms)", text: $model.timeoutMs)
                    Toggle(
                        "在回答中显示 Panel / Judge 中间结果",
                        isOn: $model.showIntermediateResults
                    )
                    Toggle(
                        "允许 Panel 使用进程内只读工具",
                        isOn: $model.panelToolsEnabled
                    )
                } else {
                    Text("时段轮转直接路由到当前模型，不运行 Panel 或 Judge。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        } footer: {
            if model.mode == .orchestration {
                Text("多模型编排仅在 Plan 模式的新用户轮次运行；后续请求直接交给 Final 模型。")
            }
        }
    }

    private func timeBinding(_ minute: Binding<Int>) -> Binding<Date> {
        Binding(
            get: {
                Calendar.current.date(
                    bySettingHour: minute.wrappedValue / 60,
                    minute: minute.wrappedValue % 60,
                    second: 0,
                    of: Date()
                ) ?? Date()
            },
            set: { date in
                let components = Calendar.current.dateComponents([.hour, .minute], from: date)
                minute.wrappedValue = (components.hour ?? 0) * 60 + (components.minute ?? 0)
            }
        )
    }

    @ViewBuilder
    private var modelOptions: some View {
        ForEach(model.options, id: \.id) { option in
            Text(option.isAvailable ? option.id : "\(option.id) \(L10n.Fusion.unavailable)")
                .tag(option.id)
        }
    }
}

private struct FusionPanelList: View {
    @ObservedObject var model: FusionSettingsModel
    let query: String

    var body: some View {
        if filteredOptions.isEmpty {
            VStack(spacing: 8) {
                Image(systemName: "rectangle.3.group")
                    .font(.title2)
                    .foregroundStyle(.secondary)
                Text(model.options.isEmpty ? L10n.Fusion.noModels : "没有匹配的模型")
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(filteredOptions.enumerated()), id: \.element.id) { index, option in
                        Button {
                            model.togglePanel(option.id)
                        } label: {
                            FusionPanelRow(
                                option: option,
                                isSelected: model.selectedPanels.contains(option.id)
                            )
                            .padding(.horizontal, 4)
                            .padding(.vertical, 8)
                        }
                        .buttonStyle(.plain)
                        .disabled(model.isBusy)
                        if index < filteredOptions.count - 1 {
                            Divider().padding(.leading, 34)
                        }
                    }
                }
            }
            .scrollIndicators(.visible)
        }
    }

    private var filteredOptions: [FusionModelOption] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return model.options }
        return model.options.filter {
            $0.id.localizedCaseInsensitiveContains(query)
                || $0.displayName.localizedCaseInsensitiveContains(query)
        }
    }
}

private struct FusionPanelRow: View {
    let option: FusionModelOption
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: isSelected ? "checkmark.circle.fill" : "circle")
                .foregroundStyle(isSelected ? Color.accentColor : Color.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(option.id)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(optionDetail)
                    .font(.caption)
                    .foregroundStyle(option.isAvailable ? Color.secondary : Color.red)
            }
            Spacer()
        }
        .contentShape(Rectangle())
    }

    private var optionDetail: String {
        option.isAvailable
            ? option.displayName
            : "\(option.displayName) · \(L10n.Fusion.unavailable)"
    }
}

final class FusionSettingsWindowController: NSWindowController, NSWindowDelegate {
    private let model: FusionSettingsModel

    init(
        loadHandler: @escaping FusionSettingsModel.LoadHandler,
        fetchModelsHandler: @escaping FusionSettingsModel.FetchModelsHandler,
        saveHandler: @escaping FusionSettingsModel.SaveHandler,
        deleteHandler: @escaping FusionSettingsModel.DeleteHandler
    ) {
        model = FusionSettingsModel(
            loadHandler: loadHandler,
            fetchModelsHandler: fetchModelsHandler,
            saveHandler: saveHandler,
            deleteHandler: deleteHandler
        )
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 900, height: 720),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Fusion 设置"
        window.minSize = NSSize(width: 760, height: 620)
        configureOpaqueWindow(window)
        window.center()
        super.init(window: window)
        window.delegate = self
        window.contentViewController = NSHostingController(
            rootView: FusionSettingsView(model: model) { [weak window] in window?.close() }
        )
        configurePersistentWindow(window)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        if let window {
            presentPersistentWindow(window)
        }
        model.reload()
    }
}

private func presentFusionAlert(title: String, message: String) {
    let alert = NSAlert()
    alert.messageText = localizedPrompt(title)
    alert.informativeText = localizedGatewayMessage(message)
    alert.alertStyle = .warning
    alert.addButton(withTitle: L10n.Common.ok)
    presentPersistentWindow(alert.window)
    alert.runModal()
    alert.window.close()
}
