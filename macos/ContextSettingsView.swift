import SwiftUI
import Charts

@MainActor
final class ContextSettingsModel: ObservableObject {
    typealias Loader = () async throws -> ContextSettings
    typealias StatsLoader = () async throws -> ContextStats
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
    private let statsLoader: StatsLoader
    private let saver: Saver
    let archiveLoader: (String) async throws -> ContextArchive
    @Published private(set) var archiveRefresh = 0
    private var loadGeneration = 0

    init(loader: @escaping Loader, statsLoader: @escaping StatsLoader, saver: @escaping Saver,
         archiveLoader: @escaping (String) async throws -> ContextArchive) {
        self.loader = loader
        self.statsLoader = statsLoader
        self.saver = saver
        self.archiveLoader = archiveLoader
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
            let settings = try await loader()
            guard generation == loadGeneration else { return }
            self.settings = settings
            draft = ContextSettingsDraft(settings: settings)
            let stats = try await statsLoader()
            guard generation == loadGeneration else { return }
            self.stats = stats
            archiveRefresh += 1
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
    @State private var selectedSession: String?
    @State private var archive: ContextArchive?
    @State private var archiveError: String?
    @State private var archiveLoading = false
    @State private var query = ""
    @State private var compressedOnly = true

    var body: some View {
        VStack(spacing: 0) {
            settingsPane
            Divider()
            HSplitView {
                statsPane
                    .frame(minWidth: 400, idealWidth: 460, maxWidth: 600)
                archivePane
                    .frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
            }
            if model.statusText != nil || model.isSaving {
                Divider()
                statusBar
            }
        }
        .frame(minWidth: 860, minHeight: 600)
        .background(Color(nsColor: .windowBackgroundColor))
        .task(id: "\(selectedSession ?? ""):\(model.archiveRefresh)") {
            archive = nil
            archiveError = nil
            archiveLoading = selectedSession != nil
            guard let session = selectedSession else { return }
            do {
                let loaded = try await model.archiveLoader(session)
                try Task.checkCancellation()
                guard selectedSession == session else { return }
                archive = loaded
            } catch is CancellationError {
                return
            } catch {
                guard !Task.isCancelled, selectedSession == session else { return }
                archiveError = localizedErrorDescription(error)
            }
            archiveLoading = false
        }
    }

    private var settingsPane: some View {
        HStack(spacing: 12) {
            Image(systemName: "square.stack.3d.up")
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(Color.accentColor)
                .frame(width: 30, height: 30)
                .background(
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .fill(Color.accentColor.opacity(0.12))
                )
            VStack(alignment: .leading, spacing: 2) {
                Text(AppLocalization.string("context.section.settings"))
                    .font(.headline)
                Text(AppLocalization.string("context.restartNote"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if model.isSaving { ProgressView().controlSize(.small) }
            Toggle(AppLocalization.string("context.enabled"), isOn: $model.draft.enabled)
                .toggleStyle(.switch)
                .controlSize(.small)
                .help(AppLocalization.string("context.restartNote"))
                .onChange(of: model.draft.enabled) { _ in
                    // A single switch needs no separate Save step.
                    guard model.canSave else { return }
                    Task { await model.save() }
                }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
        .disabled(model.settings == nil || model.isSaving)
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
                HStack(spacing: 8) {
                    NativeSearchField(text: $query, placeholder: AppLocalization.string("context.search"))
                        .accessibilityLabel(AppLocalization.string("context.search"))
                    Picker("", selection: $compressedOnly) {
                        Text(AppLocalization.string("context.filter.compressed")).tag(true)
                        Text(AppLocalization.string("context.filter.all")).tag(false)
                    }
                    .labelsHidden()
                    .pickerStyle(.segmented)
                    .fixedSize()
                }
                sessionList(stats.sessions.filter {
                    (!compressedOnly || $0.compressions > 0)
                        && (query.isEmpty || $0.display.localizedCaseInsensitiveContains(query)
                            || $0.session.localizedCaseInsensitiveContains(query))
                })
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
        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 10), count: 4), spacing: 10) {
            ConsoleMetricCard(
                title: AppLocalization.string("context.stats.sessions"),
                value: String(totals.sessions),
                symbol: "archivebox", tint: .blue
            )
            ConsoleMetricCard(
                title: AppLocalization.string("context.stats.compressions"),
                value: String(totals.compressions),
                symbol: "arrow.down.right.and.arrow.up.left", tint: .purple
            )
            ConsoleMetricCard(
                title: AppLocalization.string("context.stats.savedTokens"),
                value: formatTokenCount(totals.savedTokens),
                symbol: "leaf", tint: .green
            )
            ConsoleMetricCard(
                title: AppLocalization.string("context.stats.cacheHit"),
                value: formatContextPercent(totals.cache.hitPercent),
                symbol: "bolt", tint: .orange
            )
        }
    }

    @ViewBuilder
    private func sessionList(_ sessions: [ContextSessionStats]) -> some View {
        if sessions.isEmpty {
            emptyState
        } else {
            Table(sessions, selection: $selectedSession) {
                TableColumn(AppLocalization.string("context.stats.display")) { session in
                    Text(session.display)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .help(session.display)
                }
                .width(min: 120, ideal: 220)
                TableColumn(AppLocalization.string("context.stats.viewTokens")) { session in
                    Text(formatTokenCount(session.viewTokens))
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
                .width(64)
                TableColumn(AppLocalization.string("context.stats.compressions")) { session in
                    Text(String(session.compressions))
                        .monospacedDigit()
                        .foregroundStyle(session.compressions == 0 ? .tertiary : .primary)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
                .width(56)
                TableColumn(AppLocalization.string("context.stats.savedTokens")) { session in
                    Text(formatTokenCount(session.savedTokens))
                        .monospacedDigit()
                        .foregroundStyle(session.savedTokens == 0 ? Color.secondary : Color.green)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
                .width(68)
            }
            .tableStyle(.inset(alternatesRowBackgrounds: true))
            .scrollContentBackground(.hidden)
            .background(ConsoleCardBackground())
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        }
    }

    private var archivePane: some View {
        Group {
            if archiveLoading {
                ProgressView(AppLocalization.string("context.inspect.loading"))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let archiveError {
                VStack(spacing: 12) {
                    Image(systemName: "exclamationmark.triangle").font(.largeTitle)
                    Text(archiveError).textSelection(.enabled)
                    Button(AppLocalization.string("context.reload")) {
                        let session = selectedSession
                        selectedSession = nil
                        Task { @MainActor in
                            await Task.yield()
                            selectedSession = session
                        }
                    }
                }.padding(24)
            } else if let archive {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 20) {
                        Text(model.stats?.sessions.first(where: { $0.id == selectedSession })?.display ?? "")
                            .font(.title3.weight(.semibold)).textSelection(.enabled)
                        if let session = model.stats?.sessions.first(where: { $0.id == selectedSession }) {
                            Text(AppLocalization.string("context.inspect.tokens",
                                formatTokenCount(session.viewTokens), formatTokenCount(session.savedTokens)))
                                .font(.callout).foregroundStyle(.secondary)
                        }
                        VStack(alignment: .leading, spacing: 10) {
                            Text(AppLocalization.string("context.inspect.growth")).font(.headline)
                            growthChart(archive)
                            Text(AppLocalization.string("context.inspect.estimate"))
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Text(AppLocalization.string("context.inspect.map")).font(.headline)
                        ForEach(archive.blocks.filter { $0.superseded_by == nil }) { block in
                            compressionCard(block, archive: archive)
                        }
                        if archive.blocks.isEmpty {
                            Text(AppLocalization.string("context.inspect.noBlocks"))
                                .foregroundStyle(.secondary)
                        }
                        let remaining = archive.segments.filter { $0.kind == "message" }.count
                        if remaining > 0 {
                            Label(AppLocalization.string("context.inspect.remaining", String(remaining)), systemImage: "bubble.left")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        let history = archive.blocks.filter { $0.superseded_by != nil }
                        if !history.isEmpty {
                            DisclosureGroup(AppLocalization.string("context.inspect.history", String(history.count))) {
                                LazyVStack(alignment: .leading, spacing: 12) {
                                    ForEach(history) { block in
                                        compressionCard(block, archive: archive)
                                    }
                                }.padding(.top, 10)
                            }
                        }
                    }.padding(20)
                }
            } else {
                VStack(spacing: 10) {
                    Image(systemName: "chart.xyaxis.line").font(.system(size: 32)).foregroundStyle(.tertiary)
                    Text(AppLocalization.string("context.inspect.choose")).font(.headline)
                    Text(AppLocalization.string("context.inspect.hint"))
                        .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                }.padding(24).frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    private func compressionCard(_ block: ContextArchive.Block, archive: ContextArchive) -> some View {
        let groups = archive.groupsByBlock[block.id] ?? []
        let count = archive.referencesByBlock[block.id]?.count ?? 0
        return VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(groups.prefix(5))) { group in
                        HStack(spacing: 6) {
                            Image(systemName: group.symbol).frame(width: 16)
                            Text(group.title).lineLimit(1)
                            Spacer(minLength: 2)
                            Text("×\(group.count)").monospacedDigit()
                        }
                        .font(.caption).foregroundStyle(.secondary)
                        .help(group.tool ?? group.title)
                        .accessibilityElement(children: .combine)
                    }
                    if groups.count > 5 {
                        let otherCount = groups.dropFirst(5).reduce(0) { $0 + $1.count }
                        Label(AppLocalization.string("context.inspect.more", String(otherCount)), systemImage: "ellipsis")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }.frame(width: 124, alignment: .leading)
                Text("}")
                    .font(.system(size: 72, weight: .ultraLight))
                    .foregroundStyle(.tertiary).accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 6) {
                    Label(block.topic.isEmpty ? AppLocalization.string("context.inspect.summary") : block.topic,
                          systemImage: "doc.text.image")
                        .font(.callout.weight(.medium)).foregroundStyle(Color.accentColor).lineLimit(2)
                    Text(AppLocalization.string("context.inspect.packed", String(count)))
                        .font(.caption).foregroundStyle(.secondary)
                    Text(String(block.summary.prefix(240)))
                        .font(.caption).foregroundStyle(.secondary).lineLimit(3)
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            DisclosureGroup(AppLocalization.string("context.inspect.summary")) {
                Text(block.summary).font(.callout).textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.top, 8)
            }
            .font(.caption)
        }
        .padding(12)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
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

    private func growthChart(_ archive: ContextArchive) -> some View {
        let originalLabel = AppLocalization.string("context.inspect.original")
        let visibleLabel = AppLocalization.string("context.inspect.visible")
        return Chart {
            ForEach(archive.points) { point in
                LineMark(x: .value("Message", point.id), y: .value("KiB", point.original))
                    .foregroundStyle(by: .value("View", originalLabel))
                    .lineStyle(StrokeStyle(lineWidth: 2, dash: [5, 4]))
            }
            ForEach(archive.points) { point in
                LineMark(x: .value("Message", point.id), y: .value("KiB", point.visible))
                    .foregroundStyle(by: .value("View", visibleLabel))
            }
        }
        .chartForegroundStyleScale([originalLabel: Color.secondary, visibleLabel: Color.accentColor])
        .chartXAxisLabel(AppLocalization.string("context.inspect.position"))
        .chartYAxisLabel("KiB")
        .frame(height: 180)
        .accessibilityLabel(AppLocalization.string("context.inspect.growth"))
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
