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
                Text(AppLocalization.string("routing.title")).font(.headline)
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
                                .opacity(route.enabled ? 1 : 0.55)
                            Toggle(AppLocalization.string("routing.enabled"), isOn: Binding(
                                get: { route.enabled },
                                set: { _ in model.toggleEnabled(route) }
                            ))
                            .toggleStyle(.switch)
                            .controlSize(.mini)
                            .labelsHidden()
                            .accessibilityLabel(AppLocalization.string("routing.enabledFor", route.displayName))
                            Image(systemName: "line.3.horizontal")
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
                        Section {
                            ForEach(suggestions, id: \.model) { route in
                                suggestionRow(route)
                                    .tag(route.model)
                            }
                            .onMove { model.moveSuggestions(from: $0, to: $1) }
                        } header: {
                            Text(AppLocalization.string("routing.suggestions"))
                        }
                    }
                }
                .disabled(model.isBusy)
                .listStyle(.sidebar)
            }
        }
    }

    /// Suggestions are not running yet, so they stay one quiet line each and
    /// leave the visual weight to the routes that are configured.
    private func suggestionRow(_ route: ModelRouteDraft) -> some View {
        HStack(spacing: 10) {
            Image(systemName: route.kind.symbol)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
                .frame(width: 26, height: 26)
            Text(route.displayName)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 6)
            Button(AppLocalization.string("routing.suggestions.enable")) {
                model.adopt(route)
                model.save()
            }
            .buttonStyle(.borderless)
            .accessibilityLabel(AppLocalization.string("routing.enabledFor", route.displayName))
        }
        .padding(.vertical, 4)
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
                    .foregroundStyle(isError ? Color.red : Color.secondary)
                    .lineLimit(2)
            }
            if model.isBusy { ProgressView().controlSize(.small) }
            Spacer()
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
            Text(route.displayName.isEmpty ? AppLocalization.string("routing.untitled") : route.displayName)
                .lineLimit(1)
                .truncationMode(.middle)
                .fontWeight(isNew ? .semibold : .regular)
        }
        .help(route.summary(atMinute: currentLocalMinute()))
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 4)
    }
}

private struct ModelRouteEditor: View {
    @ObservedObject var model: ModelRoutingModel
    let kind: ModelRouteKind

    var body: some View {
        Form {
            Section {
                header
                if kind == .fusion {
                    FusionStageFlow(model: model)
                        .padding(.vertical, 8)
                } else {
                    TimelineView(.periodic(from: .now, by: 30)) { _ in
                        ModelRouteFlow(model: model, activity: activity)
                    }
                    .padding(.vertical, 8)
                }
                if kind == .failover, let activity {
                    activitySummary(activity)
                }
            }
            Section {
                TextField(AppLocalization.string("routing.field.name"), text: binding(\.profile.id, ""))
                LabeledContent(AppLocalization.string("routing.field.entry")) {
                    Text(draft.model)
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)
                }
            }
            if kind == .fusion {
                fusionAdvanced
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

    private func activitySummary(_ activity: ModelRouteActivity) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Label {
                Text(activity.served == nil
                    ? AppLocalization.string("routing.activity.failed")
                    : AppLocalization.string("routing.activity.served", activity.served ?? ""))
            } icon: {
                Image(systemName: activity.served == nil ? "exclamationmark.triangle" : "checkmark.circle")
                    .foregroundStyle(activity.served == nil ? Color.orange : kind.tint)
            }
            if let date = activity.switchedAt {
                Divider()
                HStack {
                    Text(AppLocalization.string("routing.activity.lastSwitch")).fontWeight(.medium)
                    Spacer()
                    Text(date, format: .dateTime.month().day().hour().minute().second())
                }
                Text(activity.failed.joined(separator: " → ") + " → "
                    + (activity.switchedTo ?? AppLocalization.string("routing.activity.failed")))
                    .textSelection(.enabled)
                Text(AppLocalization.string("routing.activity.reason", activity.reason))
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 8).fill(kind.tint.opacity(0.07)))
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: kind.symbol)
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(kind.tint)
                .frame(width: 34, height: 34)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(kind.tint.opacity(0.14)))
            Text(kind.title).font(.headline)
            Spacer(minLength: 8)
            if model.editingModel != nil {
                moreMenu
            }
        }
        .padding(.vertical, 2)
        .help(kind.help)
    }

    /// Destructive actions live here, away from Save, and still confirm first.
    private var moreMenu: some View {
        Menu {
            Button(role: .destructive) {
                model.deleteSelected()
            } label: {
                Label(AppLocalization.string("routing.deleteEllipsis"), systemImage: "trash")
            }
        } label: {
            Image(systemName: "ellipsis.circle")
                .font(.system(size: 15))
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(AppLocalization.string("routing.more"))
        .accessibilityLabel(AppLocalization.string("routing.more"))
    }

    private func binding<T>(_ keyPath: WritableKeyPath<ModelRouteDraft, T>, _ fallback: T) -> Binding<T> {
        Binding(
            get: { model.draft?[keyPath: keyPath] ?? fallback },
            set: { model.draft?[keyPath: keyPath] = $0 }
        )
    }

    private var draft: ModelRouteDraft { model.draft ?? ModelRouteDraft(kind: kind) }

    private var fusionAdvanced: some View {
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

/// How a failover or schedule route sends a request, drawn like a
/// switchboard: Agents, the gateway, and every model the request can reach.
/// The model rows are the editor: click a model to change it, add, reorder
/// or remove rows and edit time slots in place.
private struct ModelRouteFlow: View {
    @ObservedObject var model: ModelRoutingModel
    let activity: ModelRouteActivity?
    @Environment(\.colorScheme) private var colorScheme
    @State private var editing: Slot?
    @State private var hovered: Slot?

    private enum Slot: Hashable {
        case source
        case fallback(Int)
        case time(UUID)
        case otherTimes
    }

    private struct Target: Identifiable {
        let id: Int
        let slot: Slot
        let model: String
        let role: String
        let isActive: Bool
    }

    private let rowHeight: CGFloat = 44
    private let rowSpacing: CGFloat = 8
    private let nodeHeight: CGFloat = 104
    private let maxFallbacks = 4
    private let maxTimeRoutes = 24

    private var route: ModelRouteDraft { model.draft ?? ModelRouteDraft(kind: .failover) }

    private var targets: [Target] {
        var items: [(Slot, String, String, Bool)] = []
        switch route.kind {
        case .fusion:
            break
        case .schedule:
            let minute = currentLocalMinute()
            let active = route.profile.activeModel(atMinute: minute)
            var activeShown = false
            for slot in route.profile.timeRoutes {
                let isActive = !activeShown && slot.model == active && fusionTimeRoute(slot, contains: minute)
                activeShown = activeShown || isActive
                items.append((.time(slot.id), slot.model, "", isActive))
            }
            items.append((.otherTimes, route.profile.finalModel, AppLocalization.string("routing.diagram.otherTimes"), !activeShown))
        case .failover:
            let active = activity?.served
            items = [(.source, route.source, AppLocalization.string("routing.diagram.primary"), active == route.source)]
            items += route.fallbacks.enumerated().map {
                (.fallback($0.offset), $0.element, AppLocalization.string("routing.diagram.backup", "\($0.offset + 1)"), active == $0.element)
            }
        }
        return items.enumerated().map {
            Target(id: $0.offset, slot: $0.element.0, model: $0.element.1, role: $0.element.2, isActive: $0.element.3)
        }
    }

    private var canAdd: Bool {
        switch route.kind {
        case .fusion: return false
        case .schedule: return route.profile.timeRoutes.count < maxTimeRoutes
        case .failover: return route.fallbacks.count < maxFallbacks
        }
    }

    private func stackHeight(_ count: Int) -> CGFloat {
        CGFloat(count) * rowHeight + CGFloat(max(0, count - 1)) * rowSpacing
    }

    var body: some View {
        let targets = targets
        let targetsHeight = stackHeight(targets.count)
        let centerHeight = max(targetsHeight, nodeHeight)
        let topInset = (centerHeight - targetsHeight) / 2
        let columnHeight = topInset + stackHeight(targets.count + (canAdd ? 1 : 0))
        let tint = route.kind.tint
        HStack(alignment: .top, spacing: 0) {
            HStack(spacing: 0) {
                node {
                    Image(systemName: "square.stack.3d.up")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundStyle(.secondary)
                    Text(AppLocalization.string("routing.diagram.client")).font(.callout.weight(.semibold))
                }
                Rectangle()
                    .fill(targets.contains(where: \.isActive) ? tint : Color(nsColor: .separatorColor))
                    .frame(width: 22, height: 1)
                node(highlighted: true) {
                    Image(nsImage: NSImage(named: colorScheme == .dark ? "CodexMixinDark" : "CodexMixin") ?? NSApp.applicationIconImage)
                        .resizable()
                        .scaledToFit()
                        .frame(width: 36, height: 36)
                        .accessibilityHidden(true)
                    Text("Mixin").font(.callout.weight(.semibold))
                }
            }
            .frame(height: centerHeight)
            Canvas { context, size in
                let start = CGPoint(x: 0, y: centerHeight / 2)
                for target in targets {
                    let y = topInset + CGFloat(target.id) * (rowHeight + rowSpacing) + rowHeight / 2
                    var path = Path()
                    path.move(to: start)
                    path.addCurve(
                        to: CGPoint(x: size.width, y: y),
                        control1: CGPoint(x: size.width * 0.55, y: start.y),
                        control2: CGPoint(x: size.width * 0.45, y: y)
                    )
                    context.stroke(
                        path,
                        with: .color(target.isActive ? tint : Color(nsColor: .separatorColor)),
                        lineWidth: target.isActive ? 1.6 : 1
                    )
                }
            }
            .frame(width: 44, height: max(centerHeight, columnHeight))
            .accessibilityHidden(true)
            VStack(spacing: rowSpacing) {
                ForEach(targets) { target in
                    targetRow(target)
                }
                if canAdd {
                    addRow
                }
            }
            .padding(.top, topInset)
            .frame(maxWidth: .infinity)
        }
        .animation(.easeInOut(duration: 0.25), value: targets.filter(\.isActive).map(\.id))
    }

    private func node<Content: View>(highlighted: Bool = false, @ViewBuilder content: () -> Content) -> some View {
        VStack(spacing: 6, content: content)
            .frame(width: 96, height: highlighted ? 92 : 72)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color(nsColor: .controlBackgroundColor)))
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(highlighted ? route.kind.tint.opacity(0.7) : Color(nsColor: .separatorColor),
                                  lineWidth: highlighted ? 1.5 : 1)
            )
    }

    private func isEditing(_ slot: Slot) -> Binding<Bool> {
        Binding(get: { editing == slot }, set: { if !$0 { editing = nil } })
    }

    private func targetRow(_ target: Target) -> some View {
        let tint = route.kind.tint
        let label = routeModelLabel(target.model, options: model.options)
        let isHovered = hovered == target.slot
        return HStack(spacing: 8) {
            Circle()
                .fill(target.isActive ? tint : Color.clear)
                .overlay(Circle().strokeBorder(target.isActive ? tint : Color.secondary.opacity(0.5)))
                .frame(width: 7, height: 7)
            Button {
                editing = target.slot
            } label: {
                HStack(spacing: 7) {
                    Group {
                        if let option = label.option, let logo = routeProviderLogo(option) {
                            Image(nsImage: logo)
                        } else {
                            Image(systemName: "cpu").font(.system(size: 11)).foregroundStyle(.secondary)
                        }
                    }
                    .frame(width: 16, height: 16)
                    Text(target.model.isEmpty ? AppLocalization.string("routing.chooseModel") : label.name)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .foregroundStyle(target.model.isEmpty ? Color.secondary : label.isMissing ? Color.red : Color.primary)
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(.tertiary)
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .frame(maxWidth: .infinity, alignment: .leading)
            .help([target.model, label.detail].filter { !$0.isEmpty }.joined(separator: " · "))
            .accessibilityLabel([target.role, target.model.isEmpty ? AppLocalization.string("routing.chooseModel") : label.name]
                .filter { !$0.isEmpty }
                .joined(separator: ", "))
            .accessibilityHint(AppLocalization.string("routing.stage.change"))
            .popover(isPresented: isEditing(target.slot), arrowEdge: .bottom) {
                RouteModelChoiceList(
                    model: model,
                    title: chooserTitle(target),
                    placeholder: AppLocalization.string("routing.searchModels"),
                    selected: [target.model].filter { !$0.isEmpty },
                    limit: 1
                ) { id in
                    setModel(id, for: target.slot)
                    editing = nil
                }
            }
            trailing(target, isHovered: isHovered)
        }
        .padding(.horizontal, 10)
        .frame(height: rowHeight)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color(nsColor: .controlBackgroundColor)))
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(target.isActive ? tint.opacity(0.6) : Color(nsColor: .separatorColor))
        )
        .onHover { inside in
            if inside {
                hovered = target.slot
            } else if hovered == target.slot {
                hovered = nil
            }
        }
        .contextMenu { rowActions(target) }
    }

    @ViewBuilder
    private func trailing(_ target: Target, isHovered: Bool) -> some View {
        switch target.slot {
        case .source, .otherTimes:
            roleText(target)
        case .fallback(let index):
            HStack(spacing: 2) {
                iconButton("chevron.up", "routing.failover.moveUp") { model.moveFallback(at: index, by: -1) }
                    .disabled(index == 0)
                iconButton("chevron.down", "routing.failover.moveDown") { model.moveFallback(at: index, by: 1) }
                    .disabled(index >= route.fallbacks.count - 1)
                iconButton("trash", "routing.failover.remove") { model.removeFallback(at: index) }
            }
            .opacity(isHovered ? 1 : 0)
            roleText(target)
        case .time(let id):
            HStack(spacing: 6) {
                timePicker(id, \.startMinute)
                Text("–").foregroundStyle(.secondary)
                timePicker(id, \.endMinute)
            }
            iconButton("trash", "routing.schedule.remove") { model.removeTimeRoute(id) }
                .opacity(isHovered ? 1 : 0)
        }
    }

    private func roleText(_ target: Target) -> some View {
        Text(target.role)
            .fontWeight(target.isActive ? .semibold : .regular)
            .foregroundStyle(target.isActive ? route.kind.tint : Color.secondary)
            .lineLimit(1)
            .fixedSize()
    }

    @ViewBuilder
    private func rowActions(_ target: Target) -> some View {
        Button(AppLocalization.string("routing.stage.change")) { editing = target.slot }
        switch target.slot {
        case .fallback(let index):
            Divider()
            Button(AppLocalization.string("routing.failover.moveUp")) { model.moveFallback(at: index, by: -1) }
                .disabled(index == 0)
            Button(AppLocalization.string("routing.failover.moveDown")) { model.moveFallback(at: index, by: 1) }
                .disabled(index >= route.fallbacks.count - 1)
            Button(AppLocalization.string("routing.failover.remove"), role: .destructive) { model.removeFallback(at: index) }
        case .time(let id):
            Divider()
            Button(AppLocalization.string("routing.schedule.remove"), role: .destructive) { model.removeTimeRoute(id) }
        case .source, .otherTimes:
            EmptyView()
        }
    }

    private func iconButton(_ symbol: String, _ key: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .frame(width: 20, height: 20)
                .contentShape(Rectangle())
        }
        .buttonStyle(.borderless)
        .help(AppLocalization.string(key))
        .accessibilityLabel(AppLocalization.string(key))
    }

    private var addRow: some View {
        let isSchedule = route.kind == .schedule
        return Button {
            if isSchedule {
                model.addTimeRoute()
            } else {
                model.addFallback()
            }
            let slot: Slot? = isSchedule
                ? model.draft?.profile.timeRoutes.last.map { .time($0.id) }
                : model.draft.map { .fallback($0.fallbacks.count - 1) }
            DispatchQueue.main.async { editing = slot }
        } label: {
            Label(
                AppLocalization.string(isSchedule ? "routing.schedule.add" : "routing.failover.add"),
                systemImage: "plus"
            )
            .foregroundStyle(route.kind.tint)
            .padding(.horizontal, 10)
            .frame(maxWidth: .infinity, minHeight: rowHeight, maxHeight: rowHeight, alignment: .leading)
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                    .foregroundStyle(route.kind.tint.opacity(0.45))
            )
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
    }

    private func chooserTitle(_ target: Target) -> String {
        switch target.slot {
        case .source: return AppLocalization.string("routing.failover.source")
        case .fallback: return target.role
        case .otherTimes: return AppLocalization.string("routing.schedule.default")
        case .time(let id):
            guard let slot = route.profile.timeRoutes.first(where: { $0.id == id }) else { return "" }
            return "\(clock(slot.startMinute))–\(clock(slot.endMinute))"
        }
    }

    private func clock(_ minute: Int) -> String {
        String(format: "%02d:%02d", minute / 60 % 24, minute % 60)
    }

    private func setModel(_ id: String, for slot: Slot) {
        switch slot {
        case .source:
            model.draft?.source = id
        case .fallback(let index):
            guard model.draft?.fallbacks.indices.contains(index) == true else { return }
            model.draft?.fallbacks[index] = id
        case .time(let uuid):
            guard let index = model.draft?.profile.timeRoutes.firstIndex(where: { $0.id == uuid }) else { return }
            model.draft?.profile.timeRoutes[index].model = id
        case .otherTimes:
            model.draft?.profile.finalModel = id
        }
    }

    private func timePicker(_ id: UUID, _ keyPath: WritableKeyPath<FusionTimeRoute, Int>) -> some View {
        let minute = Binding<Int>(
            get: { model.draft?.profile.timeRoutes.first { $0.id == id }?[keyPath: keyPath] ?? 0 },
            set: { value in
                guard let index = model.draft?.profile.timeRoutes.firstIndex(where: { $0.id == id }) else { return }
                model.draft?.profile.timeRoutes[index][keyPath: keyPath] = value
            }
        )
        let date = Binding<Date>(
            get: {
                Calendar.current.date(
                    bySettingHour: minute.wrappedValue / 60 % 24,
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
        return DatePicker("", selection: date, displayedComponents: .hourAndMinute)
            .labelsHidden()
            .fixedSize()
    }
}

/// Fusion as it runs, left to right: panel models answer in parallel, the
/// judge reviews their reports, then the final model edits and replies.
/// Every node opens its model picker, so the diagram is the editor.
private struct FusionStageFlow: View {
    @ObservedObject var model: ModelRoutingModel
    @State private var editing: Stage?

    private enum Stage {
        case panels, judge, final
    }

    private let cardHeight: CGFloat = 44
    private let cardSpacing: CGFloat = 6
    private let stageWidth: CGFloat = 172
    private let connectorWidth: CGFloat = 40
    private let maxPanels = 8

    private var profile: FusionSettingsProfile { model.draft?.profile ?? FusionSettingsProfile() }
    private var tint: Color { ModelRouteKind.fusion.tint }

    var body: some View {
        let panels = profile.panelModels
        let rows = panels.count + 1
        let columnHeight = CGFloat(rows) * cardHeight + CGFloat(rows - 1) * cardSpacing
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .bottom, spacing: 0) {
                stageHeader(1, AppLocalization.string("routing.stage.panels"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                Color.clear.frame(width: connectorWidth, height: 1)
                stageHeader(2, AppLocalization.string("routing.stage.judge"))
                    .frame(width: stageWidth, alignment: .leading)
                Color.clear.frame(width: connectorWidth, height: 1)
                stageHeader(3, AppLocalization.string("routing.stage.final"))
                    .frame(width: stageWidth, alignment: .leading)
            }
            HStack(spacing: 0) {
                VStack(spacing: cardSpacing) {
                    ForEach(panels, id: \.self) { id in
                        modelCard(id, role: AppLocalization.string("routing.diagram.panel")) { editing = .panels }
                    }
                    editPanelsCard(count: panels.count)
                }
                .frame(maxWidth: .infinity)
                .popover(isPresented: isEditing(.panels), arrowEdge: .bottom) {
                    RouteModelChoiceList(
                        model: model,
                        title: AppLocalization.string("routing.fusion.panels"),
                        placeholder: AppLocalization.string("routing.fusion.searchPanels"),
                        selected: panels,
                        limit: maxPanels
                    ) { model.togglePanel($0) }
                }
                Canvas { context, size in
                    let end = CGPoint(x: size.width - 2, y: size.height / 2)
                    for index in panels.indices {
                        let y = CGFloat(index) * (cardHeight + cardSpacing) + cardHeight / 2
                        var path = Path()
                        path.move(to: CGPoint(x: 0, y: y))
                        path.addCurve(
                            to: end,
                            control1: CGPoint(x: size.width * 0.6, y: y),
                            control2: CGPoint(x: size.width * 0.4, y: end.y)
                        )
                        context.stroke(path, with: .color(tint.opacity(0.55)), lineWidth: 1.2)
                    }
                    drawArrowHead(context, at: end)
                }
                .frame(width: connectorWidth, height: columnHeight)
                .accessibilityHidden(true)
                modelCard(profile.judgeModel, role: nil) { editing = .judge }
                    .frame(width: stageWidth)
                    .popover(isPresented: isEditing(.judge), arrowEdge: .bottom) {
                        singleChoice(AppLocalization.string("routing.fusion.judge"), selection: \.profile.judgeModel)
                    }
                Canvas { context, size in
                    let y = size.height / 2
                    var path = Path()
                    path.move(to: CGPoint(x: 0, y: y))
                    path.addLine(to: CGPoint(x: size.width - 2, y: y))
                    context.stroke(path, with: .color(tint.opacity(0.55)), lineWidth: 1.2)
                    drawArrowHead(context, at: CGPoint(x: size.width - 2, y: y))
                }
                .frame(width: connectorWidth, height: cardHeight)
                .accessibilityHidden(true)
                modelCard(profile.finalModel, role: nil) { editing = .final }
                    .frame(width: stageWidth)
                    .popover(isPresented: isEditing(.final), arrowEdge: .bottom) {
                        singleChoice(AppLocalization.string("routing.fusion.final"), selection: \.profile.finalModel)
                    }
            }
        }
    }

    private func isEditing(_ stage: Stage) -> Binding<Bool> {
        Binding(get: { editing == stage }, set: { if !$0 { editing = nil } })
    }

    private func singleChoice(_ title: String, selection: WritableKeyPath<ModelRouteDraft, String>) -> some View {
        RouteModelChoiceList(
            model: model,
            title: title,
            placeholder: AppLocalization.string("routing.searchModels"),
            selected: [model.draft?[keyPath: selection] ?? ""].filter { !$0.isEmpty },
            limit: 1
        ) { id in
            model.draft?[keyPath: selection] = id
            editing = nil
        }
    }

    private func drawArrowHead(_ context: GraphicsContext, at point: CGPoint) {
        var head = Path()
        head.move(to: CGPoint(x: point.x - 6, y: point.y - 4))
        head.addLine(to: point)
        head.addLine(to: CGPoint(x: point.x - 6, y: point.y + 4))
        context.stroke(head, with: .color(tint.opacity(0.8)), style: StrokeStyle(lineWidth: 1.4, lineCap: .round, lineJoin: .round))
    }

    private func stageHeader(_ step: Int, _ title: String) -> some View {
        HStack(spacing: 6) {
            Text("\(step)")
                .font(.caption2.weight(.bold))
                .foregroundStyle(tint)
                .frame(width: 16, height: 16)
                .background(Circle().fill(tint.opacity(0.14)))
            Text(title)
                .font(.callout.weight(.semibold))
                .lineLimit(1)
        }
        .accessibilityElement(children: .combine)
    }

    private func modelCard(_ id: String, role: String?, action: @escaping () -> Void) -> some View {
        let label = routeModelLabel(id, options: model.options)
        return Button(action: action) {
            HStack(spacing: 8) {
                Group {
                    if let option = label.option, let logo = routeProviderLogo(option) {
                        Image(nsImage: logo)
                    } else {
                        Image(systemName: "cpu").font(.system(size: 11)).foregroundStyle(.secondary)
                    }
                }
                .frame(width: 16, height: 16)
                Text(id.isEmpty ? AppLocalization.string("routing.chooseModel") : label.name)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .foregroundStyle(id.isEmpty ? Color.secondary : label.isMissing ? Color.red : Color.primary)
                Spacer(minLength: 4)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 10)
            .frame(maxWidth: .infinity, minHeight: cardHeight, maxHeight: cardHeight, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color(nsColor: .controlBackgroundColor)))
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(id.isEmpty || label.isMissing ? Color.red.opacity(0.5) : tint.opacity(0.35))
            )
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
        .help([id, label.detail].filter { !$0.isEmpty }.joined(separator: " · "))
        .accessibilityLabel([role, id.isEmpty ? AppLocalization.string("routing.chooseModel") : label.name]
            .compactMap { $0 }
            .joined(separator: ", "))
        .accessibilityHint(AppLocalization.string("routing.stage.change"))
    }

    private func editPanelsCard(count: Int) -> some View {
        Button {
            editing = .panels
        } label: {
            HStack(spacing: 6) {
                Image(systemName: count == 0 ? "plus.circle" : "checklist")
                Text(AppLocalization.string("routing.stage.editPanels"))
                Spacer(minLength: 4)
            }
            .foregroundStyle(count == 0 ? Color.red : tint)
            .padding(.horizontal, 10)
            .frame(maxWidth: .infinity, minHeight: cardHeight, maxHeight: cardHeight, alignment: .leading)
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                    .foregroundStyle(tint.opacity(0.45))
            )
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
    }
}

/// Human-readable name for a model reference. Saved values keep the raw id,
/// including the `official:` prefix; only the label drops it.
private func routeModelLabel(
    _ id: String,
    options: [FusionModelOption]
) -> (name: String, detail: String, option: FusionModelOption?, isMissing: Bool) {
    let officialPrefix = "official:"
    let isOfficial = id.hasPrefix(officialPrefix)
    let bareID = isOfficial ? String(id.dropFirst(officialPrefix.count)) : id
    let official = AppLocalization.string("routing.model.official")
    if let option = options.first(where: { $0.id == id }) {
        let name = option.displayName.hasPrefix(officialPrefix)
            ? String(option.displayName.dropFirst(officialPrefix.count))
            : option.displayName
        let provider = option.providerName.isEmpty && isOfficial ? official : option.providerName
        return (option.isAvailable ? name : bareID, provider, option, !option.isAvailable)
    }
    return (bareID, isOfficial ? official : "", nil, !id.isEmpty)
}

/// Model chooser shown from a flow node: native search field, providers as
/// groups, checkboxes when several models can be chosen.
private struct RouteModelChoiceList: View {
    @ObservedObject var model: ModelRoutingModel
    let title: String
    let placeholder: String
    let selected: [String]
    let limit: Int
    let pick: (String) -> Void
    @State private var query = ""

    private var isMultiple: Bool { limit > 1 }

    var body: some View {
        let groups = model.pickerGroups(including: selected).map { group in
            (name: group.name, options: group.options.filter {
                query.isEmpty
                    || $0.id.localizedCaseInsensitiveContains(query)
                    || $0.displayName.localizedCaseInsensitiveContains(query)
                    || $0.providerName.localizedCaseInsensitiveContains(query)
            })
        }.filter { !$0.options.isEmpty }
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(title).font(.headline)
                Spacer()
                if isMultiple {
                    Text(AppLocalization.string("routing.fusion.panelCount", "\(selected.count)"))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
            NativeSearchField(text: $query, placeholder: placeholder)
                .frame(height: 24)
            ScrollView {
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
                            .foregroundStyle(.secondary)
                            .padding(.top, index == 0 ? 0 : 10)
                            .padding(.bottom, 3)
                        ForEach(groups[index].options, id: \.id) { option in
                            row(option)
                        }
                    }
                }
                .padding(.trailing, 6)
            }
        }
        .padding(14)
        .frame(width: 380, height: 420)
    }

    @ViewBuilder
    private func row(_ option: FusionModelOption) -> some View {
        let isOn = selected.contains(option.id)
        if isMultiple {
            Toggle(isOn: Binding(get: { isOn }, set: { _ in pick(option.id) })) {
                label(option)
            }
            .toggleStyle(.checkbox)
            .disabled(!isOn && selected.count >= limit)
            .padding(.vertical, 3)
            .help(option.id)
        } else {
            Button {
                pick(option.id)
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "checkmark")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(Color.accentColor)
                        .opacity(isOn ? 1 : 0)
                        .frame(width: 14)
                    label(option)
                }
                .padding(.vertical, 4)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(option.id)
            .accessibilityAddTraits(isOn ? .isSelected : [])
        }
    }

    private func label(_ option: FusionModelOption) -> some View {
        let label = routeModelLabel(option.id, options: [option])
        return HStack(spacing: 7) {
            Group {
                if let image = routeProviderLogo(option) {
                    Image(nsImage: image)
                } else {
                    Image(systemName: "cube").foregroundStyle(.secondary)
                }
            }
            .frame(width: 16, height: 16)
            Text(label.name)
                .lineLimit(1)
                .truncationMode(.middle)
                .foregroundStyle(option.isAvailable ? Color.primary : Color.red)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
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
