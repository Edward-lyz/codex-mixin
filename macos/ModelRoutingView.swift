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
        case .failover: "arrow.triangle.2.circlepath"
        }
    }

    var tint: Color {
        switch self {
        case .fusion: .purple
        case .schedule: .orange
        case .failover: .teal
        }
    }

    var pitch: String {
        switch self {
        case .fusion: AppLocalization.string("routing.kind.fusionPitch")
        case .schedule: AppLocalization.string("routing.kind.schedulePitch")
        case .failover: AppLocalization.string("routing.kind.failoverPitch")
        }
    }

    var bestFor: String {
        switch self {
        case .fusion: AppLocalization.string("routing.kind.fusionBestFor")
        case .schedule: AppLocalization.string("routing.kind.scheduleBestFor")
        case .failover: AppLocalization.string("routing.kind.failoverBestFor")
        }
    }

    /// Stages a request passes through, shown as a miniature on the start page.
    var flow: [String] {
        switch self {
        case .fusion:
            [
                AppLocalization.string("routing.flow.panels"),
                AppLocalization.string("routing.flow.judge"),
                AppLocalization.string("routing.flow.final"),
            ]
        case .schedule:
            [
                AppLocalization.string("routing.flow.clock"),
                AppLocalization.string("routing.flow.slotModel"),
                AppLocalization.string("routing.flow.defaultModel"),
            ]
        case .failover:
            [
                AppLocalization.string("routing.flow.source"),
                AppLocalization.string("routing.flow.backup1"),
                AppLocalization.string("routing.flow.backup2"),
            ]
        }
    }
}

struct ModelRouteActivity: Equatable {
    var served: String?
    var switchedAt: Date?
    var failed: [String] = []
    var switchedTo: String?
    var reason = ""

    init(dictionary: [String: Any]) {
        served = dictionary["served"] as? String
        if let last = dictionary["last_switch"] as? [String: Any] {
            switchedAt = (last["at_ms"] as? Double).map { Date(timeIntervalSince1970: $0 / 1_000) }
            failed = last["failed"] as? [String] ?? []
            switchedTo = last["served"] as? String
            reason = last["reason"] as? String ?? ""
        }
    }
}

/// One editable route, kept separate from the sidebar selection identity.
struct ModelRouteDraft: Equatable {
    var kind: ModelRouteKind
    var profile = FusionSettingsProfile()
    var source = ""
    var fallbacks: [String] = []
    var enabled = true
    var activity: ModelRouteActivity?

    init(kind: ModelRouteKind) {
        self.kind = kind
        profile.mode = kind == .schedule ? .timeRotation : .orchestration
    }

    init?(entry: [String: Any]) {
        guard let rawKind = entry["kind"] as? String, let kind = ModelRouteKind(rawValue: rawKind) else {
            return nil
        }
        self.kind = kind
        enabled = entry["enabled"] as? Bool ?? true
        activity = (entry["activity"] as? [String: Any]).map(ModelRouteActivity.init(dictionary:))
        if kind == .failover {
            source = entry["source"] as? String ?? ""
            fallbacks = entry["fallbacks"] as? [String] ?? []
            profile.id = entry["id"] as? String ?? ""
        } else {
            profile = FusionSettingsProfile(dictionary: entry)
        }
    }

    var name: String { profile.id.trimmingCharacters(in: .whitespacesAndNewlines) }

    var model: String { kind == .failover && name.isEmpty ? source : "auto/\(name)" }

    var displayName: String {
        if name.isEmpty { return kind == .failover ? source : "" }
        return "auto/\(name)"
    }

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
            if !name.isEmpty {
                object["id"] = name
            }
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
        case .failover: return (name.isEmpty ? nil : nameError(otherModels)) ?? failoverError(otherSources)
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
        if name.isEmpty && otherSources.contains(source) {
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
    typealias ChangeHandler = ([String], OperationProgress?) async throws -> Void

    private let listHandler: ListHandler
    private let modelsHandler: ModelsHandler
    private let saveHandler: SaveHandler
    private let deleteHandler: DeleteHandler
    private let changeHandler: ChangeHandler

    @Published var routes: [ModelRouteDraft] = []
    @Published var options: [FusionModelOption] = []
    @Published var selection: String?
    @Published var draft: ModelRouteDraft?
    @Published var editingModel: String?
    @Published var status = ""
    @Published var statusIsError = false
    @Published var isBusy = false
    @Published var hasLoaded = false
    @Published var suggestionOrder = UserDefaults.standard.stringArray(forKey: "modelRoutingSuggestionOrder") ?? []

    init(
        listHandler: @escaping ListHandler,
        modelsHandler: @escaping ModelsHandler,
        saveHandler: @escaping SaveHandler,
        deleteHandler: @escaping DeleteHandler,
        changeHandler: @escaping ChangeHandler
    ) {
        self.listHandler = listHandler
        self.modelsHandler = modelsHandler
        self.saveHandler = saveHandler
        self.deleteHandler = deleteHandler
        self.changeHandler = changeHandler
    }

    var validationError: String? {
        guard let draft else { return nil }
        let others = routes.filter { $0.model != editingModel }
        return draft.validationError(
            otherModels: Set(others.map(\.model)),
            otherSources: Set(others.filter { $0.kind == .failover && $0.name.isEmpty }.map(\.source))
        )
    }

    var isDirty: Bool {
        guard let draft else { return false }
        var edited = draft
        var saved = routes.first { $0.model == editingModel }
        edited.activity = nil
        saved?.activity = nil
        return edited != saved
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

    /// Picker choices grouped by provider, in provider-list order; missing references come last.
    func pickerGroups(including references: [String]) -> [(name: String, options: [FusionModelOption])] {
        var groups: [(name: String, options: [FusionModelOption])] = []
        for option in pickerOptions(including: references) {
            if groups.last?.name == option.providerName {
                groups[groups.count - 1].options.append(option)
            } else {
                groups.append((option.providerName, [option]))
            }
        }
        return groups
    }

    /// Built-in route groups: a fusion panel, a never-stuck chain across
    /// providers, and one failover per model name served by several providers.
    /// They stay suggestions until saved, because saving restarts the gateway.
    var suggestions: [ModelRouteDraft] {
        let available = options.filter(\.isAvailable)
        guard available.count > 1 else { return [] }
        var result: [ModelRouteDraft] = []
        var fusion = ModelRouteDraft(kind: .fusion)
        fusion.profile.id = "fusion"
        let picked = resolveFusionModelSelection(
            availableModelIDs: available.map(\.id),
            storedPanelModels: [],
            storedJudgeModel: "",
            storedFinalModel: ""
        )
        fusion.profile.panelModels = picked.panelModels
        fusion.profile.judgeModel = picked.judgeModel
        fusion.profile.finalModel = picked.finalModel
        result.append(fusion)
        var chain: [FusionModelOption] = []
        for option in available where !chain.contains(where: { $0.providerID == option.providerID }) {
            chain.append(option)
        }
        if chain.count > 1 {
            var neverStuck = ModelRouteDraft(kind: .failover)
            neverStuck.profile.id = "never-stuck"
            neverStuck.source = chain[0].id
            neverStuck.fallbacks = chain.dropFirst().prefix(4).map(\.id)
            result.append(neverStuck)
        }
        let sameName = Dictionary(grouping: available) { $0.displayName.lowercased() }
        for name in sameName.keys.sorted() {
            guard let group = sameName[name], Set(group.map(\.providerID)).count > 1 else { continue }
            var route = ModelRouteDraft(kind: .failover)
            route.profile.id = name.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: " ", with: "-")
            route.source = group[0].id
            route.fallbacks = group.dropFirst().prefix(4).map(\.id)
            result.append(route)
        }
        let models = Set(routes.map(\.model))
        let sources = Set(routes.filter { $0.kind == .failover && $0.name.isEmpty }.map(\.source))
        return result.filter { $0.validationError(otherModels: models, otherSources: sources) == nil }
            .sorted {
                (suggestionOrder.firstIndex(of: $0.model) ?? Int.max)
                    < (suggestionOrder.firstIndex(of: $1.model) ?? Int.max)
            }
    }

    func adopt(_ route: ModelRouteDraft) {
        selection = route.model
        editingModel = nil
        draft = route
        setStatus(nil)
    }

    func refreshActivity() async {
        guard !isBusy, hasLoaded else { return }
        do {
            let loaded = try await listHandler()
            guard !Task.isCancelled, !isBusy else { return }
            routes = loaded
        } catch {
            setStatus(localizedErrorDescription(error), isError: true)
        }
    }

    func reloadOnShow() async {
        guard !isBusy else { return }
        if hasLoaded {
            await refreshActivity()
        } else {
            await reload(keeping: selection)
        }
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
        if let model, !routes.contains(where: { $0.model == model }),
           let route = suggestions.first(where: { $0.model == model }) {
            adopt(route)
            return
        }
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
            let used = Set(routes.filter { $0.kind == .failover && $0.name.isEmpty }.map(\.source))
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
        while taken.contains("auto/\(name)") {
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

    func moveSuggestions(from offsets: IndexSet, to destination: Int) {
        var ordered = suggestions.map(\.model)
        ordered.move(fromOffsets: offsets, toOffset: destination)
        suggestionOrder = ordered
        UserDefaults.standard.set(ordered, forKey: "modelRoutingSuggestionOrder")
    }

    func moveRoutes(from offsets: IndexSet, to destination: Int) {
        guard !isBusy else { return }
        let previous = routes
        routes.move(fromOffsets: offsets, toOffset: destination)
        let models = routes.map(\.model)
        isBusy = true
        Task { @MainActor in
            defer { isBusy = false }
            do {
                try await changeHandler(["route", "reorder"] + models, nil)
                setStatus(nil)
            } catch {
                routes = previous
                setStatus(localizedErrorDescription(error), isError: true)
            }
        }
    }

    func toggleEnabled(_ route: ModelRouteDraft) {
        guard !isBusy else { return }
        isBusy = true
        Task { @MainActor in
            defer { isBusy = false }
            do {
                try await runOperationProgress(
                    title: AppLocalization.string("routing.progress.saveTitle"),
                    phases: [AppLocalization.string("routing.progress.write"),
                             AppLocalization.string("routing.progress.restart"),
                             AppLocalization.string("routing.progress.refresh"),
                             AppLocalization.string("routing.progress.done")],
                    successTitle: AppLocalization.string("routing.progress.saved"),
                    failureTitle: AppLocalization.string("routing.progress.failed")
                ) { progress in
                    try await self.changeHandler(["route", route.enabled ? "disable" : "enable", "--model", route.model], progress)
                }
                routes = try await listHandler()
                if editingModel == route.model { draft?.enabled = !route.enabled }
                setStatus(nil)
            } catch {
                setStatus(localizedErrorDescription(error), isError: true)
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
        .task {
            while !Task.isCancelled {
                do { try await Task.sleep(nanoseconds: 5_000_000_000) } catch { return }
                await model.refreshActivity()
            }
        }
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
            if model.routes.isEmpty && model.draft == nil && model.suggestions.isEmpty {
                emptySidebar
            } else {
                List(selection: Binding(get: { model.selection }, set: { model.select($0) })) {
                    ForEach(model.routes, id: \.model) { route in
                        HStack(spacing: 8) {
                            ModelRouteRow(route: route)
                            Toggle(AppLocalization.string("routing.enabled"), isOn: Binding(
                                get: { route.enabled },
                                set: { _ in model.toggleEnabled(route) }
                            ))
                            .toggleStyle(.switch)
                            .controlSize(.mini)
                            .labelsHidden()
                            .accessibilityLabel(AppLocalization.string("routing.enabledFor", route.displayName))
                            Image(systemName: "line.3.horizontal")
                                .font(.caption)
                                .foregroundStyle(.tertiary)
                        }
                        .tag(route.model)
                    }
                    .onMove { model.moveRoutes(from: $0, to: $1) }
                    if model.editingModel == nil, model.selection == nil, let draft = model.draft {
                        ModelRouteRow(route: draft, isNew: true)
                    }
                    let suggestions = model.suggestions
                    if !suggestions.isEmpty {
                        Section(AppLocalization.string("routing.suggestions")) {
                            ForEach(suggestions, id: \.model) { route in
                                HStack(spacing: 8) {
                                    ModelRouteRow(route: route)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                        .contentShape(Rectangle())
                                        .onTapGesture { model.adopt(route) }
                                    Toggle(AppLocalization.string("routing.enabled"), isOn: Binding(
                                        get: { false },
                                        set: { _ in model.adopt(route); model.save() }
                                    ))
                                    .toggleStyle(.switch)
                                    .controlSize(.mini)
                                    .labelsHidden()
                                    .accessibilityLabel(AppLocalization.string("routing.enabledFor", route.displayName))
                                    Image(systemName: "line.3.horizontal")
                                        .font(.caption)
                                        .foregroundStyle(.tertiary)
                                }
                                .tag(route.model)
                                .help(AppLocalization.string("routing.suggestions.help"))
                            }
                            .onMove { model.moveSuggestions(from: $0, to: $1) }
                        }
                    }
                }
                .disabled(model.isBusy)
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
        VStack(spacing: 8) {
            Spacer()
            Image(systemName: "point.3.connected.trianglepath.dotted")
                .font(.system(size: 26))
                .foregroundStyle(.tertiary)
            Text(AppLocalization.string("routing.empty"))
                .font(.callout.weight(.medium))
                .foregroundStyle(.secondary)
            Text(AppLocalization.string("routing.emptyHint"))
                .font(.caption)
                .foregroundStyle(.tertiary)
                .multilineTextAlignment(.center)
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
            ModelRouteGallery(model: model)
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

/// Start page shown when no route is open: explains what each route type does
/// and creates one in a single click.
private struct ModelRouteGallery: View {
    @ObservedObject var model: ModelRoutingModel

    private let columns = [GridItem(.adaptive(minimum: 210), spacing: 14, alignment: .top)]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                if !model.status.isEmpty {
                    statusBanner
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text(AppLocalization.string("routing.gallery.title"))
                        .font(.title2.weight(.semibold))
                    Text(model.routes.isEmpty
                        ? AppLocalization.string("routing.gallery.subtitle")
                        : AppLocalization.string("routing.selectHint"))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                LazyVGrid(columns: columns, alignment: .leading, spacing: 14) {
                    ForEach(ModelRouteKind.allCases) { kind in
                        ModelRouteKindCard(kind: kind, isEnabled: !model.isBusy && model.hasLoaded) {
                            model.newRoute(kind)
                        }
                    }
                }
                steps
                if model.isBusy {
                    ProgressView().controlSize(.small)
                }
            }
            .padding(28)
            .frame(maxWidth: 860, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
    }

    private var statusBanner: some View {
        HStack(spacing: 8) {
            Image(systemName: model.statusIsError ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                .foregroundStyle(model.statusIsError ? Color.red : Color.green)
            Text(model.status)
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(model.statusIsError ? Color.red.opacity(0.1) : Color.green.opacity(0.1))
        )
    }

    private var stepTexts: [String] {
        [
            AppLocalization.string("routing.steps.pick"),
            AppLocalization.string("routing.steps.save"),
            AppLocalization.string("routing.steps.use"),
        ]
    }

    private var steps: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(AppLocalization.string("routing.steps.title"))
                .font(.headline)
            HStack(alignment: .top, spacing: 14) {
                ForEach(Array(stepTexts.enumerated()), id: \.offset) { index, text in
                    HStack(alignment: .top, spacing: 8) {
                        Text("\(index + 1)")
                            .font(.caption.weight(.bold))
                            .foregroundStyle(Color.accentColor)
                            .frame(width: 20, height: 20)
                            .background(Circle().fill(Color.accentColor.opacity(0.14)))
                        Text(text)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .padding(16)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Color(nsColor: .controlBackgroundColor))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(Color(nsColor: .separatorColor))
        )
    }
}

private struct ModelRouteKindCard: View {
    let kind: ModelRouteKind
    let isEnabled: Bool
    let create: () -> Void
    @State private var isHovered = false

    var body: some View {
        Button(action: create) {
            VStack(alignment: .leading, spacing: 12) {
                Image(systemName: kind.symbol)
                    .font(.system(size: 20, weight: .medium))
                    .foregroundStyle(kind.tint)
                    .frame(width: 40, height: 40)
                    .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(kind.tint.opacity(0.14)))
                VStack(alignment: .leading, spacing: 4) {
                    Text(kind.title).font(.headline)
                    Text(kind.pitch)
                        .font(.callout)
                        .foregroundStyle(.primary.opacity(0.85))
                        .fixedSize(horizontal: false, vertical: true)
                }
                flow
                Text(kind.bestFor)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
                HStack(spacing: 4) {
                    Text(AppLocalization.string("routing.gallery.create"))
                    Image(systemName: "arrow.right")
                }
                .font(.callout.weight(.semibold))
                .foregroundStyle(isEnabled ? kind.tint : Color.secondary)
            }
            .padding(16)
            .frame(maxWidth: .infinity, minHeight: 250, alignment: .topLeading)
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(Color(nsColor: .controlBackgroundColor))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(isHovered && isEnabled ? kind.tint.opacity(0.7) : Color(nsColor: .separatorColor))
            )
            .shadow(color: .black.opacity(isHovered && isEnabled ? 0.08 : 0), radius: 8, y: 2)
            .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(!isEnabled)
        .onHover { isHovered = $0 }
        .animation(.easeOut(duration: 0.12), value: isHovered)
        .help(kind.help)
        .accessibilityLabel(AppLocalization.string("routing.gallery.createAccessibility", kind.title))
        .accessibilityHint(kind.help)
    }

    /// A miniature of how the route sends a request, e.g. Panel x N -> Judge -> Final.
    private var flow: some View {
        HStack(spacing: 4) {
            ForEach(Array(kind.flow.enumerated()), id: \.offset) { index, step in
                if index > 0 {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 8, weight: .bold))
                        .foregroundStyle(.tertiary)
                }
                Text(step)
                    .font(.caption2.weight(.medium))
                    .lineLimit(1)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(Capsule().fill(kind.tint.opacity(0.12)))
                    .foregroundStyle(kind.tint)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
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
                            .foregroundStyle(.secondary)
                    }
                }
                Text(route.summary(atMinute: currentLocalMinute()))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 4)
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
                TimelineView(.periodic(from: .now, by: 30)) { _ in
                    ModelRouteFlow(route: draft, options: model.options, activity: activity)
                }
                .padding(.vertical, 8)
                if kind == .failover {
                    activitySummary
                }
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

    private var activity: ModelRouteActivity? {
        guard let saved = model.routes.first(where: { $0.model == model.editingModel }),
              saved.source == draft.source, saved.fallbacks == draft.fallbacks else { return nil }
        return saved.activity
    }

    private var activitySummary: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label {
                Text(activity.map { $0.served == nil
                    ? AppLocalization.string("routing.activity.failed")
                    : AppLocalization.string("routing.activity.served", $0.served ?? "")
                } ?? AppLocalization.string("routing.activity.waiting"))
            } icon: {
                Image(systemName: activity == nil ? "clock" : activity?.served == nil ? "exclamationmark.triangle" : "checkmark.circle")
                    .foregroundStyle(activity != nil && activity?.served == nil ? Color.orange : kind.tint)
            }
            if let activity, let date = activity.switchedAt {
                Divider()
                HStack {
                    Text(AppLocalization.string("routing.activity.lastSwitch")).fontWeight(.medium)
                    Spacer()
                    Text(date, format: .dateTime.month().day().hour().minute().second())
                        .foregroundStyle(.secondary)
                }
                Text(activity.failed.joined(separator: " → ") + " → "
                    + (activity.switchedTo ?? AppLocalization.string("routing.activity.failed")))
                    .textSelection(.enabled)
                Text(AppLocalization.string("routing.activity.reason", activity.reason))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
        .font(.caption)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 8).fill(kind.tint.opacity(0.07)))
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
        let groups = model.pickerGroups(including: [selection.wrappedValue])
        return Picker(title, selection: selection) {
            if selection.wrappedValue.isEmpty {
                Text(AppLocalization.string("routing.chooseModel")).tag("")
            }
            ForEach(groups.indices, id: \.self) { index in
                Section {
                    ForEach(groups[index].options, id: \.id) { option in
                        Label {
                            Text(option.isAvailable
                                ? option.displayName
                                : "\(option.id) \(AppLocalization.string("fusion.unavailable"))")
                        } icon: {
                            if let image = routeProviderLogo(option) {
                                Image(nsImage: image)
                            } else {
                                Image(systemName: "cube")
                            }
                        }
                        .tag(option.id)
                    }
                } header: {
                    if !groups[index].name.isEmpty { Text(groups[index].name) }
                }
            }
        }
        .labelsHidden(labelsHidden)
        .help(selection.wrappedValue)
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
            Text(AppLocalization.string(kind == .failover ? "routing.failover.nameHelp" : "routing.field.entryHelp"))
                .font(.caption)
                .foregroundStyle(.secondary)
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
        let groups = model.pickerGroups(including: selected).map { group in
            (name: group.name, options: group.options.filter {
                panelQuery.isEmpty
                    || $0.id.localizedCaseInsensitiveContains(panelQuery)
                    || $0.displayName.localizedCaseInsensitiveContains(panelQuery)
                    || $0.providerName.localizedCaseInsensitiveContains(panelQuery)
            })
        }.filter { !$0.options.isEmpty }
        return ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                if groups.isEmpty {
                    Text(AppLocalization.string(model.options.isEmpty ? "fusion.noModels" : "routing.noMatches"))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .padding(.vertical, 24)
                        .frame(maxWidth: .infinity)
                }
                ForEach(groups.indices, id: \.self) { index in
                    Text(groups[index].name.isEmpty
                        ? AppLocalization.string("fusion.unavailableModel")
                        : groups[index].name)
                        .font(.caption2.weight(.semibold))
                        .textCase(.uppercase)
                        .foregroundStyle(.tertiary)
                        .padding(.top, index == 0 ? 2 : 12)
                        .padding(.bottom, 4)
                    ForEach(groups[index].options, id: \.id) { option in
                        panelRow(option, isOn: selected.contains(option.id), isFull: selected.count >= 8)
                    }
                }
            }
        }
        .frame(minHeight: 140, idealHeight: 220, maxHeight: 260)
    }

    func panelRow(_ option: FusionModelOption, isOn: Bool, isFull: Bool) -> some View {
        Button {
            model.togglePanel(option.id)
        } label: {
            HStack(spacing: 9) {
                Image(systemName: isOn ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(isOn ? Color.accentColor : Color.secondary)
                Group {
                    if let image = routeProviderLogo(option) {
                        Image(nsImage: image)
                    } else {
                        Image(systemName: "cube").foregroundStyle(.secondary)
                    }
                }
                .frame(width: 16, height: 16)
                Text(option.isAvailable ? option.displayName : option.id)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .foregroundStyle(option.isAvailable ? Color.primary : Color.red)
                Spacer(minLength: 8)
                Text(option.id)
                    .font(.system(.caption2, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .padding(.vertical, 5)
            .padding(.horizontal, 6)
            .background(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(isOn ? Color.primary.opacity(0.05) : Color.clear)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!isOn && isFull)
        .help(option.id)
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
        nameSection
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

/// The model's provider logo at 16 pt; menus draw images at their own size.
private func routeProviderLogo(_ option: FusionModelOption) -> NSImage? {
    guard
        !option.providerID.isEmpty,
        let image = providerLogoImage(option.providerID, websiteURL: option.providerWebsiteURL)?.copy() as? NSImage
    else { return nil }
    image.size = NSSize(width: 16, height: 16)
    return image
}

/// How a route sends a request, drawn like a switchboard: the client entry,
/// the gateway with the route type, and every model the request can reach.
private struct ModelRouteFlow: View {
    let route: ModelRouteDraft
    let options: [FusionModelOption]
    let activity: ModelRouteActivity?
    @Environment(\.colorScheme) private var colorScheme

    private struct Target: Identifiable {
        let id: Int
        let model: String
        let role: String
        let isActive: Bool
    }

    private let rowHeight: CGFloat = 46
    private let rowSpacing: CGFloat = 8

    private var targets: [Target] {
        var items: [(String, String, Bool)] = []
        switch route.kind {
        case .fusion:
            items = route.profile.panelModels.map { ($0, AppLocalization.string("routing.diagram.panel"), true) }
            items.append((route.profile.judgeModel, AppLocalization.string("routing.diagram.judge"), false))
            items.append((route.profile.finalModel, AppLocalization.string("routing.diagram.final"), false))
        case .schedule:
            let active = route.profile.activeModel(atMinute: currentLocalMinute())
            var activeShown = false
            for slot in route.profile.timeRoutes {
                let isActive = !activeShown && slot.model == active
                    && fusionTimeRoute(slot, contains: currentLocalMinute())
                activeShown = activeShown || isActive
                items.append((slot.model, "\(clock(slot.startMinute))–\(clock(slot.endMinute))", isActive))
            }
            items.append((route.profile.finalModel, AppLocalization.string("routing.diagram.otherTimes"), !activeShown))
        case .failover:
            let active = activity?.served
            items = [(route.source, AppLocalization.string("routing.diagram.primary"), active == route.source)]
            items += route.fallbacks.enumerated().map {
                ($0.element, AppLocalization.string("routing.diagram.backup", "\($0.offset + 1)"), active == $0.element)
            }
        }
        return items.enumerated().map { Target(id: $0.offset, model: $0.element.0, role: $0.element.1, isActive: $0.element.2) }
    }

    private func clock(_ minute: Int) -> String {
        String(format: "%02d:%02d", minute / 60 % 24, minute % 60)
    }

    var body: some View {
        let targets = targets
        let height = max(rowHeight, CGFloat(targets.count) * rowHeight + CGFloat(max(0, targets.count - 1)) * rowSpacing)
        HStack(spacing: 0) {
            node {
                Image(systemName: "square.stack.3d.up")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(.secondary)
                Text(AppLocalization.string("routing.diagram.client")).font(.caption.weight(.semibold))
                Text(route.model.isEmpty ? "auto/…" : route.model)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Rectangle()
                .fill(targets.contains(where: \.isActive) ? route.kind.tint : Color(nsColor: .separatorColor))
                .frame(width: 22, height: 1)
            node(highlighted: true) {
                Image(nsImage: NSImage(named: colorScheme == .dark ? "CodexMixinDark" : "CodexMixin") ?? NSApp.applicationIconImage)
                    .resizable()
                    .scaledToFit()
                    .frame(width: 36, height: 36)
                    .accessibilityHidden(true)
                Text("Mixin").font(.caption.weight(.semibold))
                Text(route.kind.title)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2)
                    .background(Capsule().fill(route.kind.tint))
            }
            Canvas { context, size in
                let start = CGPoint(x: 0, y: size.height / 2)
                for target in targets {
                    let y = CGFloat(target.id) * (rowHeight + rowSpacing) + rowHeight / 2
                    var path = Path()
                    path.move(to: start)
                    path.addCurve(
                        to: CGPoint(x: size.width, y: y),
                        control1: CGPoint(x: size.width * 0.55, y: start.y),
                        control2: CGPoint(x: size.width * 0.45, y: y)
                    )
                    context.stroke(
                        path,
                        with: .color(target.isActive ? route.kind.tint : Color(nsColor: .separatorColor)),
                        lineWidth: target.isActive ? 1.6 : 1
                    )
                }
            }
            .frame(width: 44, height: height)
            VStack(spacing: rowSpacing) {
                ForEach(targets) { target in
                    targetRow(target)
                }
            }
            .frame(maxWidth: .infinity)
        }
        .frame(minHeight: height)
        .animation(.easeInOut(duration: 0.25), value: targets.filter(\.isActive).map(\.id))
    }

    private func node<Content: View>(highlighted: Bool = false, @ViewBuilder content: () -> Content) -> some View {
        VStack(spacing: 4, content: content)
            .frame(width: 100)
            .padding(.vertical, 10)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color(nsColor: .controlBackgroundColor)))
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(highlighted ? route.kind.tint.opacity(0.7) : Color(nsColor: .separatorColor),
                                  lineWidth: highlighted ? 1.5 : 1)
            )
    }

    private func targetRow(_ target: Target) -> some View {
        let option = options.first { $0.id == target.model }
        return HStack(spacing: 7) {
            Circle()
                .fill(target.isActive ? route.kind.tint : Color.clear)
                .overlay(Circle().strokeBorder(target.isActive ? route.kind.tint : Color.secondary.opacity(0.5)))
                .frame(width: 7, height: 7)
            if let option, let logo = routeProviderLogo(option) {
                Image(nsImage: logo)
            } else {
                Image(systemName: "cpu").font(.system(size: 11)).foregroundStyle(.secondary)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(target.model.isEmpty ? AppLocalization.string("routing.chooseModel") : (option?.displayName ?? target.model))
                    .font(.callout)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .foregroundStyle(option?.isAvailable == false || option == nil && !target.model.isEmpty ? Color.red : Color.primary)
                if let option, !option.providerName.isEmpty {
                    Text(option.providerName)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Spacer(minLength: 6)
            Text(target.role)
                .font(.caption.weight(target.isActive ? .semibold : .regular))
                .foregroundStyle(target.isActive ? route.kind.tint : Color.secondary)
                .lineLimit(1)
                .fixedSize(horizontal: true, vertical: false)
        }
        .padding(.horizontal, 10)
        .frame(height: rowHeight)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color(nsColor: .controlBackgroundColor)))
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(target.isActive ? route.kind.tint.opacity(0.6) : Color(nsColor: .separatorColor))
        )
        .help(target.model)
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
