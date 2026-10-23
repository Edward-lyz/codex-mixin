import Cocoa
import SwiftUI

private let agentManagementContentWidth: CGFloat = 720
private let agentManagementContentHeight: CGFloat = 560
private let agentIconSize: CGFloat = 28

@MainActor
final class AgentManagementModel: ObservableObject {
    typealias ListHandler = () async throws -> AgentListResponse
    typealias VersionsHandler = () async throws -> AgentVersionsResponse
    typealias UpgradeHandler = (AgentEntry) async throws -> AgentUpgradeResponse
    typealias ConnectHandler = (AgentEntry, AgentConnectMode?) async throws -> Void
    typealias DisconnectHandler = (AgentEntry) async throws -> Void
    typealias ConnectModePicker = @MainActor (AgentEntry) -> AgentConnectMode?
    typealias ConfirmHandler = @MainActor (String, String) -> Bool
    typealias ErrorDescription = (Error) -> String

    struct UpgradeFailure: Equatable {
        let name: String
        let message: String
    }

    struct UpgradeBatch: Equatable {
        var total: Int
        var completed: Int
        var currentAgentName: String
    }

    enum AgentManagementStatus: Equatable {
        case message(String)
        case failure(String)

        var text: String {
            switch self {
            case .message(let text), .failure(let text): return text
            }
        }

        var isError: Bool {
            if case .failure = self { return true }
            return false
        }
    }

    @Published private(set) var entries: [AgentEntry] = []
    @Published private(set) var isReloading = false
    @Published private(set) var isCheckingVersions = false
    @Published private(set) var busyAgentIDs: Set<String> = []
    @Published private(set) var batch: UpgradeBatch?
    @Published private(set) var status: AgentManagementStatus?
    @Published private(set) var loadError: String?
    @Published private(set) var lastCheckedAt: Date?
    @Published var showsUndetectedAgents = false

    private let listHandler: ListHandler
    private let versionsHandler: VersionsHandler
    private let upgradeHandler: UpgradeHandler
    private let connectHandler: ConnectHandler
    private let disconnectHandler: DisconnectHandler
    private let connectModePicker: ConnectModePicker
    private let confirmHandler: ConfirmHandler
    private let errorDescription: ErrorDescription

    init(
        listHandler: @escaping ListHandler,
        versionsHandler: @escaping VersionsHandler,
        upgradeHandler: @escaping UpgradeHandler,
        connectHandler: @escaping ConnectHandler,
        disconnectHandler: @escaping DisconnectHandler,
        connectModePicker: @escaping ConnectModePicker = { _ in nil },
        confirmHandler: @escaping ConfirmHandler = { _, _ in false },
        errorDescription: @escaping ErrorDescription = { localizedErrorDescription($0) }
    ) {
        self.listHandler = listHandler
        self.versionsHandler = versionsHandler
        self.upgradeHandler = upgradeHandler
        self.connectHandler = connectHandler
        self.disconnectHandler = disconnectHandler
        self.connectModePicker = connectModePicker
        self.confirmHandler = confirmHandler
        self.errorDescription = errorDescription
    }

    var detectedEntries: [AgentEntry] {
        detectedAgentEntries(entries)
    }

    var undetectedEntries: [AgentEntry] {
        undetectedAgentEntries(entries)
    }

    var upgradableEntries: [AgentEntry] {
        upgradableAgentEntries(entries)
    }

    var upgradableCount: Int {
        upgradableEntries.count
    }

    var isOperating: Bool {
        !busyAgentIDs.isEmpty || batch != nil
    }

    var isBusy: Bool {
        isReloading || isOperating
    }

    var showsUpdateAllButton: Bool {
        upgradableCount >= 2 || batch != nil
    }

    var canUpgradeAll: Bool {
        batch == nil && !isBusy && upgradableCount >= 2
    }

    var undetectedFoldTitle: String {
        agentUndetectedFoldTitle(count: undetectedEntries.count)
    }

    var lastCheckedText: String? {
        lastCheckedAt.map(agentLastCheckedText)
    }

    func entry(withID id: String) -> AgentEntry? {
        entries.first { $0.id == id }
    }

    func isBusy(agentID: String) -> Bool {
        busyAgentIDs.contains(agentID)
    }

    func rowPresentation(for entry: AgentEntry) -> AgentRowPresentation {
        agentRowPresentation(for: entry, isBusy: isBusy(agentID: entry.id))
    }

    func toggleUndetectedAgents() {
        showsUndetectedAgents.toggle()
    }

    func refresh() async {
        await reload()
        await checkVersions()
    }

    func reload() async {
        guard !isReloading else { return }
        isReloading = true
        defer { isReloading = false }
        do {
            let response = try await listHandler()
            entries = orderedAgentEntries(response.agents)
            loadError = nil
        } catch {
            let message = errorDescription(error)
            loadError = message
            status = .failure(AppLocalization.string("agentManagement.loadFailed", message))
        }
    }

    func checkVersions() async {
        guard !isCheckingVersions else { return }
        isCheckingVersions = true
        defer { isCheckingVersions = false }
        do {
            let response = try await versionsHandler()
            let cliByID = Dictionary(
                response.agents.map { ($0.id, $0.cli) },
                uniquingKeysWith: { first, _ in first }
            )
            entries = entries.map { entry in
                guard let cli = cliByID[entry.id] else { return entry }
                return entry.replacingCLI(cli)
            }
            lastCheckedAt = Date()
        } catch {
            status = .failure(
                AppLocalization.string("agentManagement.versionsFailed", errorDescription(error))
            )
        }
    }

    func upgrade(agentID: String) async {
        guard !isBusy, let entry = entry(withID: agentID), entry.cli.update else { return }
        let outcome = await performUpgrade(entry: entry)
        if let response = outcome.response {
            status = .message(
                AppLocalization.string(
                    "agentManagement.upgradeSucceeded",
                    entry.name,
                    response.cli.version
                )
            )
        } else {
            status = .failure(
                AppLocalization.string(
                    "agentManagement.upgradeFailed",
                    entry.name,
                    outcome.errorMessage ?? ""
                )
            )
        }
    }

    func upgradeAll() async {
        guard canUpgradeAll else { return }
        let targets = upgradableEntries
        batch = UpgradeBatch(
            total: targets.count,
            completed: 0,
            currentAgentName: targets[0].name
        )
        var upgraded: [String] = []
        var failures: [UpgradeFailure] = []
        for (index, entry) in targets.enumerated() {
            batch?.completed = index
            batch?.currentAgentName = entry.name
            let outcome = await performUpgrade(entry: entry)
            if let response = outcome.response {
                upgraded.append("\(entry.name) \(response.cli.version)")
            } else {
                failures.append(
                    UpgradeFailure(name: entry.name, message: outcome.errorMessage ?? "")
                )
            }
        }
        let total = targets.count
        batch = nil
        let separator = AppLocalization.string("agentManagement.listSeparator")
        let summary = AppLocalization.string(
            "agentManagement.updateSummary",
            String(upgraded.count),
            String(total),
            upgraded.joined(separator: separator)
        )
        if failures.isEmpty {
            status = .message(summary)
        } else {
            let failureText = failures
                .map { "\($0.name)：\($0.message)" }
                .joined(separator: separator)
            status = .failure(
                "\(summary) · "
                    + AppLocalization.string("agentManagement.updateSummaryFailures", failureText)
            )
        }
    }

    func connect(agentID: String) async {
        guard !isBusy, let entry = entry(withID: agentID) else { return }
        var mode: AgentConnectMode?
        if agentRequiresConnectMode(id: entry.id) {
            // Cancelling the mode panel leaves the agent untouched.
            guard let picked = connectModePicker(entry) else {
                // The switch already moved; re-render it from the CLI state.
                objectWillChange.send()
                return
            }
            mode = picked
        }
        busyAgentIDs.insert(agentID)
        status = nil
        do {
            try await connectHandler(entry, mode)
            await reload()
            busyAgentIDs.remove(agentID)
            status = .message(
                AppLocalization.string(
                    "agentManagement.connectedRestart",
                    entry.name,
                    entry.name
                )
            )
        } catch {
            busyAgentIDs.remove(agentID)
            status = .failure(
                AppLocalization.string(
                    "agentManagement.connectFailed",
                    entry.name,
                    errorDescription(error)
                )
            )
        }
    }

    func disconnect(agentID: String) async {
        guard !isBusy,
              let entry = entry(withID: agentID),
              entry.integration.state == .managed
        else { return }
        let confirmation = agentDisconnectConfirmation(for: entry)
        guard confirmHandler(confirmation.title, confirmation.message) else {
            // The switch already moved; re-render it from the CLI state.
            objectWillChange.send()
            return
        }
        busyAgentIDs.insert(agentID)
        status = nil
        do {
            try await disconnectHandler(entry)
            await reload()
            busyAgentIDs.remove(agentID)
            status = .message(
                AppLocalization.string(
                    "agentManagement.disconnectedRestart",
                    entry.name,
                    entry.name
                )
            )
        } catch {
            busyAgentIDs.remove(agentID)
            status = .failure(
                AppLocalization.string(
                    "agentManagement.disconnectFailed",
                    entry.name,
                    errorDescription(error)
                )
            )
        }
    }

    private struct UpgradeOutcome {
        let response: AgentUpgradeResponse?
        let errorMessage: String?
    }

    private func performUpgrade(entry: AgentEntry) async -> UpgradeOutcome {
        busyAgentIDs.insert(entry.id)
        defer { busyAgentIDs.remove(entry.id) }
        do {
            let response = try await upgradeHandler(entry)
            let rowID = entries.contains(where: { $0.id == response.id }) ? response.id : entry.id
            if let index = entries.firstIndex(where: { $0.id == rowID }) {
                entries[index] = entries[index].replacingCLI(response.cli)
            }
            return UpgradeOutcome(response: response, errorMessage: nil)
        } catch {
            return UpgradeOutcome(response: nil, errorMessage: errorDescription(error))
        }
    }
}

private struct AgentManagementView: View {
    @ObservedObject var model: AgentManagementModel
    /// True inside the console: SwiftUI toolbars of a child controller are not
    /// bridged into the window, so the controls sit above the list.
    var embedded = false

    var body: some View {
        VStack(spacing: 0) {
            legacyToolbar
            content
            footer
        }
        .frame(
            minWidth: agentManagementContentWidth,
            minHeight: agentManagementContentHeight
        )
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                toolbarControls
            }
        }
    }

    /// macOS 13 cannot bridge SwiftUI toolbars into an AppKit window, so the
    /// same controls sit above the list there.
    @ViewBuilder
    private var legacyToolbar: some View {
        if #available(macOS 14.0, *), !embedded {
            EmptyView()
        } else {
            HStack(spacing: 10) {
                Spacer(minLength: 0)
                toolbarControls
            }
            .padding(.horizontal, 20)
            .padding(.top, 10)
        }
    }

    @ViewBuilder
    private var toolbarControls: some View {
        toolbarProgress
        if model.showsUpdateAllButton {
            Button {
                Task { await model.upgradeAll() }
            } label: {
                Label(
                    AppLocalization.string("agentManagement.updateAll"),
                    systemImage: "arrow.down.circle"
                )
            }
            .disabled(!model.canUpgradeAll)
            .keyboardShortcut("u", modifiers: [.command, .shift])
            .help(AppLocalization.string("agentManagement.updateAllHelp"))
        }
        Button {
            Task { await model.refresh() }
        } label: {
            Label(
                AppLocalization.string("agentManagement.refresh"),
                systemImage: "arrow.clockwise"
            )
        }
        .disabled(model.isBusy)
        .keyboardShortcut("r", modifiers: .command)
        .help(AppLocalization.string("agentManagement.refreshHelp"))
    }

    @ViewBuilder
    private var toolbarProgress: some View {
        if let batch = model.batch {
            progressLabel(AppLocalization.string(
                "agentManagement.updatingAllProgress",
                batch.currentAgentName,
                String(batch.completed + 1),
                String(batch.total)
            ))
        } else if model.isCheckingVersions {
            progressLabel(AppLocalization.string("agentManagement.checkingVersions"))
        } else if model.isReloading {
            progressLabel(AppLocalization.string("agentManagement.reloading"))
        }
    }

    private func progressLabel(_ text: String) -> some View {
        HStack(spacing: 6) {
            ProgressView().controlSize(.small)
            Text(text)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    // A grouped Form draws the native inset, rounded list that System
    // Settings uses; on macOS 26 it picks up the Liquid Glass materials.
    @ViewBuilder
    private var content: some View {
        if model.entries.isEmpty {
            emptyState
        } else {
            Form {
                Section {
                    ForEach(model.detectedEntries, id: \.id) { entry in
                        AgentRowView(model: model, entry: entry)
                    }
                }
                if !model.undetectedEntries.isEmpty {
                    Section {
                        if model.showsUndetectedAgents {
                            ForEach(model.undetectedEntries, id: \.id) { entry in
                                AgentRowView(model: model, entry: entry)
                            }
                        }
                    } header: {
                        undetectedHeader
                    }
                }
            }
            .formStyle(.grouped)
            .animation(.easeInOut(duration: 0.18), value: model.showsUndetectedAgents)
        }
    }

    private var undetectedHeader: some View {
        Button {
            model.toggleUndetectedAgents()
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .rotationEffect(.degrees(model.showsUndetectedAgents ? 90 : 0))
                Text(model.undetectedFoldTitle)
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
    }

    private var emptyState: some View {
        VStack(spacing: 10) {
            if let loadError = model.loadError {
                Image(systemName: "exclamationmark.triangle")
                    .font(.system(size: 28))
                    .foregroundStyle(.orange)
                Text(AppLocalization.string("agentManagement.loadFailed", loadError))
                    .font(.callout)
                    .multilineTextAlignment(.center)
                    .textSelection(.enabled)
            } else if model.isReloading {
                ProgressView()
            } else {
                Image(systemName: "person.2")
                    .font(.system(size: 28))
                    .foregroundStyle(.secondary)
                Text(AppLocalization.string("agentManagement.emptyTitle"))
                    .font(.headline)
                Text(AppLocalization.string("agentManagement.emptyHint"))
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder
    private var footer: some View {
        if model.status != nil || model.lastCheckedText != nil {
            HStack(alignment: .center, spacing: 8) {
                if let status = model.status {
                    Image(
                        systemName: status.isError
                            ? "exclamationmark.triangle.fill"
                            : "checkmark.circle.fill"
                    )
                    .foregroundStyle(status.isError ? Color.orange : Color.green)
                    Text(status.text)
                        .font(.caption)
                        .lineLimit(2)
                        .textSelection(.enabled)
                }
                Spacer(minLength: 8)
                if let lastCheckedText = model.lastCheckedText {
                    Text(lastCheckedText)
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 8)
        }
    }
}

private struct AgentRowView: View {
    @ObservedObject var model: AgentManagementModel
    let entry: AgentEntry

    var body: some View {
        let presentation = model.rowPresentation(for: entry)
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                AgentLogoView(agentID: entry.id)
                VStack(alignment: .leading, spacing: 3) {
                    titleLine(presentation)
                    if entry.integration.state != .unsupported {
                        summaryLine(presentation)
                    }
                }
                Spacer(minLength: 12)
                updateControl(presentation)
                Toggle("", isOn: connectBinding)
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .controlSize(.small)
                    .disabled(model.isBusy || entry.integration.state == .unsupported)
                    .help(connectHelp(presentation))
                    .accessibilityLabel(connectHelp(presentation))
            }
            .padding(.vertical, 4)
        }
        .accessibilityElement(children: .contain)
    }

    private func titleLine(_ presentation: AgentRowPresentation) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(entry.name)
                .font(.body)
            if let cliVersionText = presentation.cliVersionText {
                Text(cliVersionText)
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
            } else if entry.cli.isMissing && presentation.appVersionText == nil {
                Text(AppLocalization.string("agentManagement.cliMissing"))
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            if let appVersionText = presentation.appVersionText {
                Text(appVersionText)
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
        .lineLimit(1)
    }

    private func summaryLine(_ presentation: AgentRowPresentation) -> some View {
        HStack(spacing: 6) {
            Circle()
                .fill(statusColor(presentation))
                .frame(width: 6, height: 6)
            Text([presentation.statusText, agentIntegrationModeText(entry.integration.mode)]
                .compactMap { $0 }.filter { !$0.isEmpty }
                .joined(separator: AppLocalization.string("agentManagement.detailSeparator")))
                .font(.callout)
                .foregroundStyle(presentation.statusIsError ? Color.orange : Color.secondary)
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .help(summaryHelp(presentation))
    }

    @ViewBuilder
    private func updateControl(_ presentation: AgentRowPresentation) -> some View {
        if presentation.isUpdating {
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text(AppLocalization.string("agentManagement.updating"))
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        } else if let updateText = presentation.updateText {
            Button {
                Task { await model.upgrade(agentID: entry.id) }
            } label: {
                Label(updateText, systemImage: "arrow.down.circle")
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(model.isBusy)
            .help(presentation.updateHelp ?? "")
        }
    }

    private var connectBinding: Binding<Bool> {
        Binding(
            get: { entry.integration.state == .managed || entry.integration.state == .drift },
            set: { isOn in
                Task { @MainActor in
                    if isOn {
                        await model.connect(agentID: entry.id)
                    } else {
                        await model.disconnect(agentID: entry.id)
                    }
                }
            }
        )
    }

    private func summaryText(_ presentation: AgentRowPresentation) -> String {
        let separator = AppLocalization.string("agentManagement.detailSeparator")
        return [presentation.statusText, presentation.detailText]
            .filter { !$0.isEmpty }
            .joined(separator: separator)
    }

    private func summaryHelp(_ presentation: AgentRowPresentation) -> String {
        [summaryText(presentation), entry.cli.path, entry.cli.command]
            .filter { !$0.isEmpty }
            .joined(separator: "\n")
    }

    private func statusColor(_ presentation: AgentRowPresentation) -> Color {
        if presentation.statusIsError { return .orange }
        return presentation.isConnected ? .green : Color(nsColor: .tertiaryLabelColor)
    }

    private func connectHelp(_ presentation: AgentRowPresentation) -> String {
        if presentation.isConnected {
            return AppLocalization.string("agentManagement.disconnectHelp", entry.name)
        }
        if entry.integration.state == .unsupported {
            return entry.integration.detail
        }
        return AppLocalization.string("agentManagement.connectHelp", entry.name)
    }
}

/// The agent's brand mark from the bundled AgentLogos folder, or an SF Symbol
/// when the bundle has none for it.
private struct AgentLogoView: View {
    let agentID: String

    var body: some View {
        Group {
            if let image = agentLogoImage(agentID) {
                Image(nsImage: image)
                    .resizable()
                    .interpolation(.high)
                    .aspectRatio(contentMode: .fit)
            } else {
                Image(systemName: "terminal")
                    .font(.system(size: 18))
                    .foregroundStyle(.secondary)
            }
        }
        .frame(width: agentIconSize, height: agentIconSize)
        .accessibilityHidden(true)
    }
}

@MainActor
private var agentLogoCache: [String: NSImage] = [:]

private struct AgentLogoAsset: Decodable {
    let file: String
    let template: Bool
}

@MainActor
private var agentLogoManifest: [String: AgentLogoAsset]?

@MainActor
private func agentLogoImage(_ agentID: String) -> NSImage? {
    if let cached = agentLogoCache[agentID] { return cached }
    let directories = [
        Bundle.main.resourceURL?.appendingPathComponent("AgentLogos", isDirectory: true),
        Bundle.main.bundleURL.appendingPathComponent("AgentLogos", isDirectory: true),
    ]
    for directory in directories.compactMap({ $0 }) {
        if agentLogoManifest == nil {
            do {
                let data = try Data(contentsOf: directory.appendingPathComponent("manifest.json"))
                agentLogoManifest = try JSONDecoder().decode([String: AgentLogoAsset].self, from: data)
            } catch {
                NSLog("Cannot load Agent logo manifest: %@", error.localizedDescription)
                continue
            }
        }
        guard let asset = agentLogoManifest?[agentID],
              let image = NSImage(contentsOf: directory.appendingPathComponent(asset.file))
        else { continue }
        image.isTemplate = asset.template
        agentLogoCache[agentID] = image
        return image
    }
    return nil
}

/// Agent management as a console page.
func makeAgentManagementPageController(model: AgentManagementModel) -> NSViewController {
    NSHostingController(rootView: AnyView(AgentManagementView(model: model, embedded: true)))
}

final class AgentManagementWindowController: NSWindowController, NSWindowDelegate {
    let model: AgentManagementModel

    init(model: AgentManagementModel) {
        self.model = model
        let window = NSWindow(
            contentRect: NSRect(
                x: 0,
                y: 0,
                width: agentManagementContentWidth,
                height: agentManagementContentHeight
            ),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = AppLocalization.string("agentManagement.windowTitle")
        window.minSize = NSSize(
            width: agentManagementContentWidth,
            height: agentManagementContentHeight
        )
        window.toolbarStyle = .unified
        configureOpaqueWindow(window)
        window.center()
        super.init(window: window)
        window.delegate = self
        let hostingController = NSHostingController(
            rootView: AgentManagementView(model: model)
        )
        if #available(macOS 14.0, *) {
            // Let the SwiftUI toolbar become the window's native toolbar.
            hostingController.sceneBridgingOptions = [.toolbars]
        }
        window.contentViewController = hostingController
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
        Task { @MainActor [weak self] in
            await self?.model.refresh()
        }
    }
}
