import SwiftUI

@MainActor
final class ContextSettingsModel: ObservableObject {
    typealias Loader = () async throws -> (ContextSettings, ContextStats)
    typealias Saver = (_ arguments: [String]) async throws -> ContextSettings

    @Published private(set) var settings: ContextSettings?
    @Published private(set) var stats: ContextStats?
    @Published private(set) var isLoading = false
    @Published private(set) var isSaving = false
    @Published private(set) var loadError: String?
    @Published private(set) var saveFailure: ContextSettingsError?
    @Published private(set) var statusMessage: String?
    @Published var draft = ContextSettingsDraft()

    private let loader: Loader
    private let saver: Saver
    private var loadGeneration = 0

    init(loader: @escaping Loader, saver: @escaping Saver) {
        self.loader = loader
        self.saver = saver
    }

    var validationIssue: ContextSettingsValidationIssue? {
        if case .failure(let issue) = draft.validated() { return issue }
        return nil
    }

    var validationMessage: String? {
        guard let issue = validationIssue else { return nil }
        switch issue {
        case .growthThreshold:
            return AppLocalization.string("context.validation.growthThreshold")
        case .pressureRatio:
            return AppLocalization.string("context.validation.pressureRatio")
        case .urgentRatio:
            return AppLocalization.string("context.validation.urgentRatio")
        case .ratioOrder:
            return AppLocalization.string("context.validation.ratioOrder")
        case .protectedMessages:
            return AppLocalization.string("context.validation.protectedMessages")
        case .protectedTokens:
            return AppLocalization.string("context.validation.protectedTokens")
        case .maxInternalRounds:
            return AppLocalization.string("context.validation.maxInternalRounds")
        case .contextWindowTokens:
            return AppLocalization.string("context.validation.contextWindowTokens")
        }
    }

    /// Arguments for `context config set`; empty when nothing changed.
    var changedArguments: [String] {
        guard let settings, case .success(let edited) = draft.validated() else { return [] }
        return contextSettingsArguments(from: settings, to: edited)
    }

    var canSave: Bool {
        settings != nil && !isLoading && !isSaving && validationIssue == nil
            && !changedArguments.isEmpty
    }

    var statusText: String? {
        if let saveFailure { return saveFailure.description }
        if let loadError { return loadError }
        if let validationMessage { return validationMessage }
        return statusMessage
    }

    var statusDetail: String? {
        guard case .cli(let info) = saveFailure else { return nil }
        var parts = [
            AppLocalization.string("context.error.code", info.code),
        ]
        if let stage = info.stage {
            parts.append(AppLocalization.string("context.error.stage", stage))
        }
        switch info.committed {
        case .some(true):
            parts.append(AppLocalization.string("context.error.committedTrue"))
        case .some(false):
            parts.append(AppLocalization.string("context.error.committedFalse"))
        case .none:
            parts.append(AppLocalization.string("context.error.committedUnknown"))
        }
        return parts.joined(separator: " · ")
    }

    var statusIsError: Bool {
        saveFailure != nil || loadError != nil
    }

    var statusSymbol: String {
        if statusIsError { return "exclamationmark.triangle.fill" }
        if validationIssue != nil { return "exclamationmark.circle.fill" }
        if statusMessage != nil { return "checkmark.circle.fill" }
        return "info.circle"
    }

    /// Draft edits that differ from the saved settings. Reverting to the saved
    /// values counts as clean again, so a show does not discard nothing.
    var hasUnsavedChanges: Bool {
        guard let settings, case .success(let edited) = draft.validated() else { return true }
        return !contextSettingsArguments(from: settings, to: edited).isEmpty
    }

    /// The console calls this each time the page is shown. A dirty form is left
    /// alone because a silent reload would discard the edits; the Reload
    /// button always reloads.
    func reloadOnShow() async {
        guard settings == nil || !hasUnsavedChanges else { return }
        await reload()
    }

    /// A newer reload supersedes older ones, so a slow first load cannot
    /// overwrite a fresh one. One reload is one pair of CLI reads.
    func reload() async {
        loadGeneration += 1
        let generation = loadGeneration
        isLoading = true
        do {
            let (settings, stats) = try await loader()
            guard generation == loadGeneration else { return }
            self.settings = settings
            self.stats = stats
            draft = ContextSettingsDraft(settings: settings)
            loadError = nil
            saveFailure = nil
            statusMessage = nil
        } catch {
            guard generation == loadGeneration else { return }
            loadError = localizedErrorDescription(error)
        }
        isLoading = false
    }

    func save() async {
        guard let settings, case .success(let edited) = draft.validated() else { return }
        let arguments = contextSettingsArguments(from: settings, to: edited)
        guard !arguments.isEmpty else {
            saveFailure = nil
            statusMessage = AppLocalization.string("context.status.unchanged")
            return
        }
        isSaving = true
        saveFailure = nil
        statusMessage = nil
        do {
            let updated = try await saver(arguments)
            self.settings = updated
            draft = ContextSettingsDraft(settings: updated)
            statusMessage = AppLocalization.string("context.status.saved")
        } catch {
            // The CLI reported the failure; never claim the update rolled back.
            saveFailure = ContextSettingsError.from(error)
        }
        isSaving = false
    }
}

struct ContextSettingsView: View {
    @ObservedObject var model: ContextSettingsModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .top, spacing: 0) {
                settingsPane
                Divider()
                statsPane
            }
            Divider()
            statusBar
        }
        .frame(minWidth: 860, minHeight: 600)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var settingsPane: some View {
        Form {
            Section {
                Toggle(AppLocalization.string("context.enabled"), isOn: $model.draft.enabled)
                    .toggleStyle(.switch)
                    .help(AppLocalization.string("context.restartNote"))
            } header: {
                Text(AppLocalization.string("context.section.settings"))
            }
            Section {
                TextField(
                    AppLocalization.string("context.growthThreshold"),
                    text: $model.draft.growthThresholdTokens
                )
                TextField(
                    AppLocalization.string("context.pressureRatio"),
                    text: $model.draft.pressureRatio
                )
                TextField(
                    AppLocalization.string("context.urgentRatio"),
                    text: $model.draft.urgentRatio
                )
                TextField(
                    AppLocalization.string("context.protectedMessages"),
                    text: $model.draft.protectedMessages
                )
                TextField(
                    AppLocalization.string("context.protectedTokens"),
                    text: $model.draft.protectedTokens
                )
                TextField(
                    AppLocalization.string("context.maxInternalRounds"),
                    text: $model.draft.maxInternalRounds
                )
                Toggle(
                    AppLocalization.string("context.followModelWindow"),
                    isOn: $model.draft.followsModelWindow
                )
                TextField(
                    AppLocalization.string("context.contextWindowTokens"),
                    text: $model.draft.contextWindowTokens
                )
                .disabled(model.draft.followsModelWindow)
            } header: {
                Text(AppLocalization.string("context.section.limits"))
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .disabled(model.settings == nil || model.isSaving)
        .frame(minWidth: 360, idealWidth: 420, maxWidth: 460, maxHeight: .infinity)
    }

    private var statsPane: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Text(AppLocalization.string("context.section.stats"))
                    .font(.title3.weight(.semibold))
                if model.isLoading { ProgressView().controlSize(.small) }
                Spacer()
                Button {
                    Task { await model.reload() }
                } label: {
                    Label(
                        AppLocalization.string("context.reload"),
                        systemImage: "arrow.clockwise"
                    )
                }
                .keyboardShortcut("r", modifiers: .command)
                .disabled(model.isLoading || model.isSaving)
            }
            if let stats = model.stats {
                metrics(stats.totals)
                sessionList(stats.sessions)
            } else if model.loadError == nil {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                Spacer()
            }
        }
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func metrics(_ totals: ContextStatsTotals) -> some View {
        HStack(spacing: 10) {
            metric(
                AppLocalization.string("context.stats.sessions"),
                String(totals.sessions)
            )
            metric(
                AppLocalization.string("context.stats.compressions"),
                String(totals.compressions)
            )
            metric(
                AppLocalization.string("context.stats.savedTokens"),
                formatTokenCount(totals.savedTokens)
            )
            metric(
                AppLocalization.string("context.stats.cacheHit"),
                formatContextPercent(totals.cache.hitPercent)
            )
        }
    }

    private func metric(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            Text(value)
                .font(.title3.weight(.semibold).monospacedDigit())
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private func sessionList(_ sessions: [ContextSessionStats]) -> some View {
        if sessions.isEmpty {
            emptyState
        } else {
            Table(sessions) {
                TableColumn(AppLocalization.string("context.stats.display")) { session in
                    Text(session.display).font(.callout)
                }
                .width(min: 120, ideal: 200)
                TableColumn(AppLocalization.string("context.stats.compressions")) { session in
                    Text(String(session.compressions)).monospacedDigit()
                }
                .width(min: 70, ideal: 90)
                TableColumn(AppLocalization.string("context.stats.savedTokens")) { session in
                    Text(formatTokenCount(session.savedTokens)).monospacedDigit()
                }
                .width(min: 80, ideal: 100)
                TableColumn(AppLocalization.string("context.stats.cacheHit")) { session in
                    Text(formatContextPercent(session.cache.hitPercent)).monospacedDigit()
                }
                .width(min: 70, ideal: 90)
            }
        }
    }

    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "archivebox")
                .font(.system(size: 30))
                .foregroundStyle(.secondary)
            Text(AppLocalization.string("context.empty.title")).font(.headline)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
    }

    private var statusBar: some View {
        HStack(spacing: 8) {
            Image(systemName: model.statusSymbol)
                .foregroundStyle(statusColor)
            VStack(alignment: .leading, spacing: 2) {
                if let text = model.statusText {
                    Text(text)
                        .font(.caption)
                        .foregroundStyle(model.statusIsError ? Color.red : Color.secondary)
                        .lineLimit(3)
                        .textSelection(.enabled)
                }
                if let detail = model.statusDetail {
                    Text(detail)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .textSelection(.enabled)
                }
            }
            Spacer()
            if model.isSaving { ProgressView().controlSize(.small) }
            Button(AppLocalization.string("context.save")) {
                Task { await model.save() }
            }
            .keyboardShortcut(.defaultAction)
            .disabled(!model.canSave)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Color(nsColor: .controlBackgroundColor))
    }

    private var statusColor: Color {
        if model.statusIsError { return .red }
        if model.validationIssue != nil { return .orange }
        if model.statusMessage != nil { return .green }
        return .secondary
    }
}
