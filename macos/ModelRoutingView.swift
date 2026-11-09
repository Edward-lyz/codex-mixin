import Cocoa
import SwiftUI

enum ModelRouteKind: String, CaseIterable, Identifiable {
    case fusion
    case schedule
    case failover

    var id: String { rawValue }

    var title: String {
        switch self {
        case .fusion: AppLocalization.string("routing.kind.fusion")
        case .schedule: AppLocalization.string("routing.kind.schedule")
        case .failover: AppLocalization.string("routing.kind.failover")
        }
    }

    var help: String {
        switch self {
        case .fusion: AppLocalization.string("routing.kind.fusionHelp")
        case .schedule: AppLocalization.string("routing.kind.scheduleHelp")
        case .failover: AppLocalization.string("routing.kind.failoverHelp")
        }
    }

    var symbol: String {
        switch self {
        case .fusion: "rectangle.3.group"
        case .schedule: "clock"
        case .failover: "arrow.triangle.branch"
        }
    }

    var tint: Color {
        switch self {
        case .fusion: .purple
        case .schedule: .orange
        case .failover: .teal
        }
    }
}

/// One editable route. Fusion and schedule routes wrap a Fusion profile and expose
/// `mixin/fusion/<id>`; failover routes attach to an existing model.
struct ModelRouteDraft: Equatable {
    var kind: ModelRouteKind
    var profile = FusionSettingsProfile()
    var source = ""
    var fallbacks: [String] = []

    init(kind: ModelRouteKind) {
        self.kind = kind
        profile.mode = kind == .schedule ? .timeRotation : .orchestration
    }

    init?(entry: [String: Any]) {
        guard let rawKind = entry["kind"] as? String, let kind = ModelRouteKind(rawValue: rawKind) else {
            return nil
        }
        self.kind = kind
        if kind == .failover {
            source = entry["source"] as? String ?? ""
            fallbacks = entry["fallbacks"] as? [String] ?? []
        } else {
            profile = FusionSettingsProfile(dictionary: entry)
        }
    }

    var name: String { profile.id.trimmingCharacters(in: .whitespacesAndNewlines) }

    var model: String { kind == .failover ? source : "mixin/fusion/\(name)" }

    var displayName: String { kind == .failover ? source : name }

    var referencedModels: [String] {
        kind == .failover ? [source] + fallbacks : profile.referencedModels
    }

    func summary(atMinute minute: Int) -> String {
        switch kind {
        case .fusion:
            AppLocalization.string("routing.summary.fusion", "\(profile.panelModels.count)", profile.finalModel)
        case .schedule:
            AppLocalization.string("routing.summary.schedule", profile.activeModel(atMinute: minute))
        case .failover:
            AppLocalization.string("routing.summary.failover", fallbacks.joined(separator: " → "))
        }
    }

    func jsonString() throws -> String {
        var object: [String: Any]
        if kind == .failover {
            object = ["source": source, "fallbacks": fallbacks]
        } else {
            var normalized = profile
            normalized.id = name
            normalized.mode = kind == .schedule ? .timeRotation : .orchestration
            object = normalized.dictionary
        }
        object["kind"] = kind.rawValue
        let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        guard let json = String(data: data, encoding: .utf8) else {
            throw FusionSettingsError.message(AppLocalization.string("routing.error.encode"))
        }
        return json
    }

    func validationError(otherModels: Set<String>, otherSources: Set<String>) -> String? {
        switch kind {
        case .fusion: return nameError(otherModels) ?? fusionError
        case .schedule: return nameError(otherModels) ?? scheduleError
        case .failover: return failoverError(otherSources)
        }
    }

    private func nameError(_ otherModels: Set<String>) -> String? {
        if name.isEmpty || name.contains("/") {
            return AppLocalization.string("routing.error.name")
        }
        if otherModels.contains(model) {
            return AppLocalization.string("routing.error.duplicate", model)
        }
        return nil
    }

    private var fusionError: String? {
        let panels = profile.panelModels
        if panels.isEmpty || panels.count > 8 {
            return AppLocalization.string("routing.error.panels")
        }
        if profile.judgeModel.isEmpty || profile.finalModel.isEmpty {
            return AppLocalization.string("routing.error.judgeFinal")
        }
        if profile.minSuccessful < 1 || profile.minSuccessful > panels.count {
            return AppLocalization.string("routing.error.minSuccessful")
        }
        if profile.timeoutMs <= 0 {
            return AppLocalization.string("routing.error.timeout")
        }
        return nil
    }

    private var scheduleError: String? {
        if profile.finalModel.isEmpty {
            return AppLocalization.string("routing.error.defaultModel")
        }
        return fusionTimeRouteValidationError(profile.timeRoutes)
    }

    private func failoverError(_ otherSources: Set<String>) -> String? {
        if source.isEmpty {
            return AppLocalization.string("routing.error.source")
        }
        if otherSources.contains(source) {
            return AppLocalization.string("routing.error.duplicate", source)
        }
        if fallbacks.isEmpty || fallbacks.count > 4 {
            return AppLocalization.string("routing.error.fallbackCount")
        }
        if fallbacks.contains(where: \.isEmpty) {
            return AppLocalization.string("routing.error.fallbackEmpty")
        }
        if Set(fallbacks).count != fallbacks.count || fallbacks.contains(source) {
            return AppLocalization.string("routing.error.fallbackRepeat")
        }
        return nil
    }
}

func currentLocalMinute() -> Int {
    let components = Calendar.current.dateComponents([.hour, .minute], from: Date())
    return (components.hour ?? 0) * 60 + (components.minute ?? 0)
}

@MainActor
final class ModelRoutingModel: ObservableObject {
    typealias ListHandler = () async throws -> [ModelRouteDraft]
    typealias ModelsHandler = () async throws -> [FusionModelOption]
    typealias SaveHandler = (String, String?, OperationProgress) async throws -> Void
    typealias DeleteHandler = (String, OperationProgress) async throws -> Void

    private let listHandler: ListHandler
    private let modelsHandler: ModelsHandler
    private let saveHandler: SaveHandler
    private let deleteHandler: DeleteHandler

    @Published var routes: [ModelRouteDraft] = []
    @Published var options: [FusionModelOption] = []
    @Published var selection: String?
    @Published var draft: ModelRouteDraft?
    @Published var editingModel: String?
    @Published var status = ""
    @Published var statusIsError = false
    @Published var isBusy = false
    @Published var hasLoaded = false

    init(
        listHandler: @escaping ListHandler,
        modelsHandler: @escaping ModelsHandler,
        saveHandler: @escaping SaveHandler,
        deleteHandler: @escaping DeleteHandler
    ) {
        self.listHandler = listHandler
        self.modelsHandler = modelsHandler
        self.saveHandler = saveHandler
        self.deleteHandler = deleteHandler
    }

    var validationError: String? {
        guard let draft else { return nil }
        let others = routes.filter { $0.model != editingModel }
        return draft.validationError(
            otherModels: Set(others.filter { $0.kind != .failover }.map(\.model)),
            otherSources: Set(others.filter { $0.kind == .failover }.map(\.source))
        )
    }

    var isDirty: Bool {
        guard let draft else { return false }
        return draft != routes.first { $0.model == editingModel }
    }

    var canSave: Bool { !isBusy && isDirty && validationError == nil }

    /// Model choices for a picker, keeping configured references that disappeared from the catalog.
    func pickerOptions(including references: [String]) -> [FusionModelOption] {
        let known = Set(options.map(\.id))
        let missing = Set(references.filter { !$0.isEmpty && !known.contains($0) })
        return options + missing.sorted().map {
            FusionModelOption(id: $0, displayName: AppLocalization.string("fusion.unavailableModel"), isAvailable: false)
        }
    }

    func reloadOnShow() async {
        guard !isBusy else { return }
        await reload(keeping: selection)
    }

    func reload(keeping model: String?) async {
        isBusy = true
        defer { isBusy = false }
        do {
            async let loadedRoutes = listHandler()
            async let loadedOptions = modelsHandler()
            routes = try await loadedRoutes
            options = try await loadedOptions
            hasLoaded = true
            setStatus(nil)
            select(routes.contains { $0.model == model } ? model : routes.first?.model)
        } catch {
            setStatus(localizedErrorDescription(error), isError: true)
        }
    }

    func select(_ model: String?) {
        selection = model
        editingModel = model
        draft = routes.first { $0.model == model }
    }

    func newRoute(_ kind: ModelRouteKind) {
        var route = ModelRouteDraft(kind: kind)
        let ids = options.filter(\.isAvailable).map(\.id)
        switch kind {
        case .fusion:
            route.profile.id = uniqueName("fusion")
            let picked = resolveFusionModelSelection(
                availableModelIDs: ids,
                storedPanelModels: [],
                storedJudgeModel: "",
                storedFinalModel: ""
            )
            route.profile.panelModels = picked.panelModels
            route.profile.judgeModel = picked.judgeModel
            route.profile.finalModel = picked.finalModel
        case .schedule:
            route.profile.id = uniqueName("schedule")
            route.profile.finalModel = ids.first ?? ""
            route.profile.timeRoutes = [FusionTimeRoute(startMinute: 9 * 60, endMinute: 18 * 60, model: ids.dropFirst().first ?? ids.first ?? "")]
        case .failover:
            let used = Set(routes.filter { $0.kind == .failover }.map(\.source))
            route.source = ids.first { !used.contains($0) } ?? ""
            route.fallbacks = ids.first { $0 != route.source }.map { [$0] } ?? []
        }
        selection = nil
        editingModel = nil
        draft = route
        setStatus(nil)
    }

    private func uniqueName(_ base: String) -> String {
        let taken = Set(routes.map(\.model))
        var name = base
        var index = 2
        while taken.contains("mixin/fusion/\(name)") {
            name = "\(base)-\(index)"
            index += 1
        }
        return name
    }

    func discardChanges() {
        if editingModel == nil {
            select(routes.first?.model)
        } else {
            select(editingModel)
        }
    }

    func togglePanel(_ id: String) {
        guard var route = draft else { return }
        if let index = route.profile.panelModels.firstIndex(of: id) {
            route.profile.panelModels.remove(at: index)
        } else if route.profile.panelModels.count < 8 {
            route.profile.panelModels.append(id)
        }
        route.profile.minSuccessful = min(max(route.profile.minSuccessful, 1), max(route.profile.panelModels.count, 1))
        draft = route
    }

    func addTimeRoute() {
        guard var route = draft, route.profile.timeRoutes.count < 24 else { return }
        let start = route.profile.timeRoutes.last?.endMinute ?? 9 * 60
        let model = options.first(where: \.isAvailable)?.id ?? route.profile.finalModel
        route.profile.timeRoutes.append(FusionTimeRoute(startMinute: start, endMinute: (start + 4 * 60) % 1_440, model: model))
        draft = route
    }

    func removeTimeRoute(_ id: UUID) {
        draft?.profile.timeRoutes.removeAll { $0.id == id }
    }

    func addFallback() {
        guard var route = draft, route.fallbacks.count < 4 else { return }
        let used = Set([route.source] + route.fallbacks)
        route.fallbacks.append(options.first { $0.isAvailable && !used.contains($0.id) }?.id ?? "")
        draft = route
    }

    func removeFallback(at index: Int) {
        guard draft?.fallbacks.indices.contains(index) == true else { return }
        draft?.fallbacks.remove(at: index)
    }

    func moveFallback(at index: Int, by offset: Int) {
        guard var route = draft, route.fallbacks.indices.contains(index),
              route.fallbacks.indices.contains(index + offset) else { return }
        route.fallbacks.swapAt(index, index + offset)
        draft = route
    }

    private func setStatus(_ message: String?, isError: Bool = false) {
        status = message ?? ""
        statusIsError = isError
    }
}

extension ModelRoutingModel {
    func save() {
        guard canSave, let route = draft else { return }
        let json: String
        do {
            json = try route.jsonString()
        } catch {
            status = localizedErrorDescription(error)
            statusIsError = true
            return
        }
        let replaced = editingModel
        isBusy = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: AppLocalization.string("routing.progress.saveTitle"),
                    phases: [
                        AppLocalization.string("routing.progress.write"),
                        AppLocalization.string("routing.progress.restart"),
                        AppLocalization.string("routing.progress.refresh"),
                        AppLocalization.string("routing.progress.done"),
                    ],
                    successTitle: AppLocalization.string("routing.progress.saved"),
                    failureTitle: AppLocalization.string("routing.progress.failed")
                ) { progress in
                    try await self.saveHandler(json, replaced, progress)
                }
                isBusy = false
                await reload(keeping: route.model)
                status = AppLocalization.string("routing.status.saved", route.model)
                statusIsError = false
            } catch {
                isBusy = false
                status = AppLocalization.string("routing.status.saveFailed", localizedErrorDescription(error))
                statusIsError = true
            }
        }
    }

    func deleteSelected() {
        guard !isBusy, let model = editingModel, confirm(
            title: AppLocalization.string("routing.deleteConfirmTitle"),
            message: AppLocalization.string("routing.deleteConfirmMessage", model)
        ) else { return }
        isBusy = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: AppLocalization.string("routing.progress.deleteTitle"),
                    phases: [
                        AppLocalization.string("routing.progress.delete"),
                        AppLocalization.string("routing.progress.restart"),
                        AppLocalization.string("routing.progress.refresh"),
                        AppLocalization.string("routing.progress.done"),
                    ],
                    successTitle: AppLocalization.string("routing.progress.deleted"),
                    failureTitle: AppLocalization.string("routing.progress.failed")
                ) { progress in
                    try await self.deleteHandler(model, progress)
                }
                isBusy = false
                await reload(keeping: nil)
                status = AppLocalization.string("routing.status.deleted", model)
                statusIsError = false
            } catch {
                isBusy = false
                status = AppLocalization.string("routing.status.deleteFailed", localizedErrorDescription(error))
                statusIsError = true
            }
        }
    }
}

struct ModelRoutingView: View {
    @ObservedObject var model: ModelRoutingModel

    var body: some View {
        HSplitView {
            sidebar
                .frame(minWidth: 250, idealWidth: 280, maxWidth: 360)
            detail
                .frame(minWidth: 560, maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 860, minHeight: 600)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var sidebar: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "point.3.filled.connected.trianglepath.dotted")
                    .font(.system(size: 18, weight: .medium))
                    .foregroundStyle(Color.accentColor)
                    .frame(width: 30, height: 30)
                    .background(RoundedRectangle(cornerRadius: 7, style: .continuous).fill(Color.accentColor.opacity(0.12)))
                VStack(alignment: .leading, spacing: 2) {
                    Text(AppLocalization.string("routing.title")).font(.headline)
                    Text(AppLocalization.string("routing.subtitle"))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
                Spacer(minLength: 4)
                newRouteMenu
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            Divider()
            if model.routes.isEmpty && model.draft == nil {
                emptySidebar
            } else {
                List(selection: Binding(get: { model.selection }, set: { model.select($0) })) {
                    ForEach(model.routes, id: \.model) { route in
                        ModelRouteRow(route: route).tag(Optional(route.model))
                    }
                    if model.editingModel == nil, let draft = model.draft {
                        ModelRouteRow(route: draft, isNew: true)
                    }
                }
                .listStyle(.sidebar)
            }
        }
    }

    private var newRouteMenu: some View {
        Menu {
            ForEach(ModelRouteKind.allCases) { kind in
                Button {
                    model.newRoute(kind)
                } label: {
                    Label(kind.title, systemImage: kind.symbol)
                }
            }
        } label: {
            Image(systemName: "plus")
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .disabled(model.isBusy || !model.hasLoaded)
        .help(AppLocalization.string("routing.new"))
        .accessibilityLabel(AppLocalization.string("routing.new"))
    }

    private var emptySidebar: some View {
        VStack(spacing: 10) {
            Spacer()
            Image(systemName: "point.3.connected.trianglepath.dotted")
                .font(.system(size: 28))
                .foregroundStyle(.tertiary)
            Text(AppLocalization.string("routing.empty")).font(.callout).foregroundStyle(.secondary)
            ForEach(ModelRouteKind.allCases) { kind in
                Button {
                    model.newRoute(kind)
                } label: {
                    Label(kind.title, systemImage: kind.symbol).frame(width: 140)
                }
                .disabled(model.isBusy || !model.hasLoaded)
            }
            Spacer()
        }
        .frame(maxWidth: .infinity)
        .padding()
    }

    @ViewBuilder
    private var detail: some View {
        if let draft = model.draft {
            VStack(spacing: 0) {
                ModelRouteEditor(model: model, kind: draft.kind)
                Divider()
                bottomBar
            }
        } else {
            VStack(spacing: 12) {
                if model.isBusy {
                    ProgressView().controlSize(.small)
                } else {
                    Text(model.status.isEmpty ? AppLocalization.string("routing.selectHint") : model.status)
                        .foregroundStyle(model.statusIsError ? Color.red : Color.secondary)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var bottomBar: some View {
        HStack(spacing: 10) {
            let message = model.validationError ?? model.status
            if !message.isEmpty {
                let isError = model.validationError != nil || model.statusIsError
                Image(systemName: isError ? "exclamationmark.triangle" : "checkmark.circle")
                    .foregroundStyle(isError ? Color.red : Color.green)
                Text(message)
                    .font(.caption)
                    .foregroundStyle(isError ? Color.red : Color.secondary)
                    .lineLimit(2)
            }
            if model.isBusy { ProgressView().controlSize(.small) }
            Spacer()
            if model.editingModel != nil {
                Button(role: .destructive) {
                    model.deleteSelected()
                } label: {
                    Text(AppLocalization.string("routing.delete"))
                }
                .disabled(model.isBusy)
            }
            Button(AppLocalization.string("routing.discard")) { model.discardChanges() }
                .disabled(model.isBusy || !model.isDirty)
            Button(AppLocalization.string("routing.save")) { model.save() }
                .liquidGlassProminentButton()
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canSave)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
        .background(Color(nsColor: .controlBackgroundColor))
    }
}

private struct ModelRouteRow: View {
    let route: ModelRouteDraft
    var isNew = false

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: route.kind.symbol)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(route.kind.tint)
                .frame(width: 26, height: 26)
                .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(route.kind.tint.opacity(0.14)))
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(route.displayName.isEmpty ? AppLocalization.string("routing.untitled") : route.displayName)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    if isNew {
                        Text(AppLocalization.string("routing.newBadge"))
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(Color.accentColor)
                    }
                }
                Text(route.summary(atMinute: currentLocalMinute()))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
        .padding(.vertical, 3)
    }
}

private struct ModelRouteEditor: View {
    @ObservedObject var model: ModelRoutingModel
    let kind: ModelRouteKind
    @State private var panelQuery = ""

    var body: some View {
        Form {
            Section {
                header
            }
            switch kind {
            case .fusion: fusionSections
            case .schedule: scheduleSections
            case .failover: failoverSections
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .disabled(model.isBusy)
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: kind.symbol)
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(kind.tint)
                .frame(width: 34, height: 34)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(kind.tint.opacity(0.14)))
            VStack(alignment: .leading, spacing: 3) {
                Text(kind.title).font(.headline)
                Text(kind.help).font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 2)
    }

    private func binding<T>(_ keyPath: WritableKeyPath<ModelRouteDraft, T>, _ fallback: T) -> Binding<T> {
        Binding(
            get: { model.draft?[keyPath: keyPath] ?? fallback },
            set: { model.draft?[keyPath: keyPath] = $0 }
        )
    }

    private var draft: ModelRouteDraft { model.draft ?? ModelRouteDraft(kind: kind) }

    private func modelPicker(_ title: String, selection: Binding<String>, labelsHidden: Bool = false) -> some View {
        Picker(title, selection: selection) {
            if selection.wrappedValue.isEmpty {
                Text(AppLocalization.string("routing.chooseModel")).tag("")
            }
            ForEach(model.pickerOptions(including: [selection.wrappedValue]), id: \.id) { option in
                Text(option.isAvailable ? option.id : "\(option.id) \(AppLocalization.string("fusion.unavailable"))")
                    .tag(option.id)
            }
        }
        .labelsHidden(labelsHidden)
    }

    private var nameSection: some View {
        Section {
            TextField(AppLocalization.string("routing.field.name"), text: binding(\.profile.id, ""))
            LabeledContent(AppLocalization.string("routing.field.entry")) {
                Text(draft.model)
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
            }
        } footer: {
            Text(AppLocalization.string("routing.field.entryHelp")).font(.caption).foregroundStyle(.secondary)
        }
    }
}

private extension View {
    @ViewBuilder
    func labelsHidden(_ hidden: Bool) -> some View {
        if hidden { labelsHidden() } else { self }
    }
}

private extension ModelRouteEditor {
    @ViewBuilder
    var fusionSections: some View {
        nameSection
        Section {
            TextField(AppLocalization.string("routing.fusion.searchPanels"), text: $panelQuery)
            panelList
        } header: {
            Text(AppLocalization.string("routing.fusion.panels"))
        } footer: {
            Text(AppLocalization.string("routing.fusion.panelCount", "\(draft.profile.panelModels.count)"))
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        Section(AppLocalization.string("routing.fusion.answer")) {
            modelPicker(AppLocalization.string("routing.fusion.judge"), selection: binding(\.profile.judgeModel, ""))
            modelPicker(AppLocalization.string("routing.fusion.final"), selection: binding(\.profile.finalModel, ""))
        }
        Section {
            DisclosureGroup(AppLocalization.string("routing.fusion.advanced")) {
                Stepper(
                    AppLocalization.string("routing.fusion.minSuccessful", "\(draft.profile.minSuccessful)"),
                    value: binding(\.profile.minSuccessful, 1),
                    in: 1...max(draft.profile.panelModels.count, 1)
                )
                Stepper(
                    AppLocalization.string("routing.fusion.timeout", "\(draft.profile.timeoutMs / 1_000)"),
                    value: Binding(
                        get: { draft.profile.timeoutMs / 1_000 },
                        set: { model.draft?.profile.timeoutMs = $0 * 1_000 }
                    ),
                    in: 10...3_600,
                    step: 10
                )
                Toggle(AppLocalization.string("routing.fusion.showIntermediate"), isOn: binding(\.profile.showIntermediateResults, true))
                Toggle(AppLocalization.string("routing.fusion.panelTools"), isOn: binding(\.profile.panelToolsEnabled, true))
            }
        } footer: {
            Text(AppLocalization.string("routing.fusion.footer")).font(.caption).foregroundStyle(.secondary)
        }
    }

    var panelList: some View {
        let selected = draft.profile.panelModels
        let options = model.pickerOptions(including: selected).filter {
            panelQuery.isEmpty
                || $0.id.localizedCaseInsensitiveContains(panelQuery)
                || $0.displayName.localizedCaseInsensitiveContains(panelQuery)
        }
        return ScrollView {
            LazyVStack(spacing: 0) {
                if options.isEmpty {
                    Text(AppLocalization.string(model.options.isEmpty ? "fusion.noModels" : "routing.noMatches"))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .padding(.vertical, 24)
                }
                ForEach(options, id: \.id) { option in
                    let isOn = selected.contains(option.id)
                    Button {
                        model.togglePanel(option.id)
                    } label: {
                        HStack(spacing: 10) {
                            Image(systemName: isOn ? "checkmark.circle.fill" : "circle")
                                .foregroundStyle(isOn ? Color.accentColor : Color.secondary)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(option.id).lineLimit(1).truncationMode(.middle)
                                Text(option.displayName)
                                    .font(.caption)
                                    .foregroundStyle(option.isAvailable ? Color.secondary : Color.red)
                                    .lineLimit(1)
                            }
                            Spacer()
                        }
                        .padding(.vertical, 5)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(!isOn && selected.count >= 8)
                    Divider().padding(.leading, 26)
                }
            }
        }
        .frame(minHeight: 140, idealHeight: 220, maxHeight: 260)
    }

    @ViewBuilder
    var scheduleSections: some View {
        nameSection
        Section {
            modelPicker(AppLocalization.string("routing.schedule.default"), selection: binding(\.profile.finalModel, ""))
            ForEach(draft.profile.timeRoutes) { route in
                timeRouteRow(route.id)
            }
            Button {
                model.addTimeRoute()
            } label: {
                Label(AppLocalization.string("routing.schedule.add"), systemImage: "plus")
            }
            .disabled(draft.profile.timeRoutes.count >= 24)
        } header: {
            Text(AppLocalization.string("routing.schedule.slots"))
        } footer: {
            VStack(alignment: .leading, spacing: 4) {
                Text(AppLocalization.string("routing.schedule.now", draft.profile.activeModel(atMinute: currentLocalMinute())))
                Text(AppLocalization.string("routing.schedule.footer"))
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
    }

    func timeRouteRow(_ id: UUID) -> some View {
        let index = draft.profile.timeRoutes.firstIndex { $0.id == id }
        func field<T>(_ keyPath: WritableKeyPath<FusionTimeRoute, T>, _ fallback: T) -> Binding<T> {
            Binding(
                get: { index.flatMap { model.draft?.profile.timeRoutes[safe: $0]?[keyPath: keyPath] } ?? fallback },
                set: { value in
                    guard let index, model.draft?.profile.timeRoutes.indices.contains(index) == true else { return }
                    model.draft?.profile.timeRoutes[index][keyPath: keyPath] = value
                }
            )
        }
        return HStack(spacing: 8) {
            DatePicker("", selection: timeBinding(field(\.startMinute, 0)), displayedComponents: .hourAndMinute)
                .labelsHidden()
                .frame(width: 92)
            Text(AppLocalization.string("routing.schedule.to")).foregroundStyle(.secondary)
            DatePicker("", selection: timeBinding(field(\.endMinute, 0)), displayedComponents: .hourAndMinute)
                .labelsHidden()
                .frame(width: 92)
            modelPicker("", selection: field(\.model, ""), labelsHidden: true)
                .frame(maxWidth: .infinity)
            Button {
                model.removeTimeRoute(id)
            } label: {
                Image(systemName: "trash")
            }
            .buttonStyle(.borderless)
            .help(AppLocalization.string("routing.schedule.remove"))
        }
    }

    func timeBinding(_ minute: Binding<Int>) -> Binding<Date> {
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
}

private extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}

private extension ModelRouteEditor {
    @ViewBuilder
    var failoverSections: some View {
        Section {
            modelPicker(AppLocalization.string("routing.failover.source"), selection: binding(\.source, ""))
        } footer: {
            Text(AppLocalization.string("routing.failover.sourceHelp")).font(.caption).foregroundStyle(.secondary)
        }
        Section {
            ForEach(Array(draft.fallbacks.indices), id: \.self) { index in
                fallbackRow(index)
            }
            Button {
                model.addFallback()
            } label: {
                Label(AppLocalization.string("routing.failover.add"), systemImage: "plus")
            }
            .disabled(draft.fallbacks.count >= 4)
        } header: {
            Text(AppLocalization.string("routing.failover.fallbacks"))
        } footer: {
            Text(AppLocalization.string("routing.failover.footer")).font(.caption).foregroundStyle(.secondary)
        }
    }

    func fallbackRow(_ index: Int) -> some View {
        let selection = Binding(
            get: { model.draft?.fallbacks[safe: index] ?? "" },
            set: { value in
                guard model.draft?.fallbacks.indices.contains(index) == true else { return }
                model.draft?.fallbacks[index] = value
            }
        )
        return HStack(spacing: 8) {
            Text("\(index + 1)")
                .font(.caption.monospacedDigit().weight(.semibold))
                .foregroundStyle(.secondary)
                .frame(width: 18)
            modelPicker("", selection: selection, labelsHidden: true)
                .frame(maxWidth: .infinity)
            Button {
                model.moveFallback(at: index, by: -1)
            } label: {
                Image(systemName: "chevron.up")
            }
            .buttonStyle(.borderless)
            .disabled(index == 0)
            .help(AppLocalization.string("routing.failover.moveUp"))
            Button {
                model.moveFallback(at: index, by: 1)
            } label: {
                Image(systemName: "chevron.down")
            }
            .buttonStyle(.borderless)
            .disabled(index >= draft.fallbacks.count - 1)
            .help(AppLocalization.string("routing.failover.moveDown"))
            Button {
                model.removeFallback(at: index)
            } label: {
                Image(systemName: "trash")
            }
            .buttonStyle(.borderless)
            .help(AppLocalization.string("routing.failover.remove"))
        }
    }
}

/// Parses `route list --json` into editable drafts.
func parseModelRoutes(_ rawJSON: String) throws -> [ModelRouteDraft] {
    guard
        let envelope = try JSONSerialization.jsonObject(with: Data(rawJSON.utf8)) as? [String: Any],
        let routes = envelope["routes"] as? [[String: Any]]
    else {
        throw FusionSettingsError.message(AppLocalization.string("routing.error.invalidJSON"))
    }
    return routes.compactMap(ModelRouteDraft.init(entry:))
}
