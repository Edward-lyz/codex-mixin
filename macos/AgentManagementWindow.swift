import Cocoa
import SwiftUI

private let agentManagementContentWidth: CGFloat = 780
private let agentManagementContentHeight: CGFloat = 540
private let agentManagementRowHeight: CGFloat = 56

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
    @Published var expandedAgentIDs: Set<String> = []
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

    func isExpanded(agentID: String) -> Bool {
        expandedAgentIDs.contains(agentID)
    }

    func rowPresentation(for entry: AgentEntry) -> AgentRowPresentation {
        agentRowPresentation(for: entry, isBusy: isBusy(agentID: entry.id))
    }

    func toggleDetails(agentID: String) {
        if expandedAgentIDs.contains(agentID) {
            expandedAgentIDs.remove(agentID)
        } else {
            expandedAgentIDs.insert(agentID)
        }
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

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
            Divider()
            footer
        }
        .frame(
            minWidth: agentManagementContentWidth,
            minHeight: agentManagementContentHeight
        )
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(AppLocalization.string("agentManagement.windowTitle"))
                    .font(.title3.weight(.semibold))
                Text(AppLocalization.string("agentManagement.subtitle"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            if let batch = model.batch {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text(AppLocalization.string(
                        "agentManagement.updatingAllProgress",
                        batch.currentAgentName,
                        String(batch.completed + 1),
                        String(batch.total)
                    ))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
            } else if model.isCheckingVersions {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text(AppLocalization.string("agentManagement.checkingVersions"))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } else if model.isReloading {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text(AppLocalization.string("agentManagement.reloading"))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Button {
                Task { await model.refresh() }
            } label: {
                Label(
                    AppLocalization.string("agentManagement.refresh"),
                    systemImage: "arrow.triangle.2.circlepath"
                )
            }
            .disabled(model.isBusy)
            .keyboardShortcut("r", modifiers: .command)
            .help(AppLocalization.string("agentManagement.refreshHelp"))
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
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    @ViewBuilder
    private var content: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                if model.entries.isEmpty, let loadError = model.loadError {
                    messageState(
                        symbol: "exclamationmark.triangle.fill",
                        title: AppLocalization.string("agentManagement.loadFailed", loadError)
                    )
                } else if model.entries.isEmpty {
                    messageState(
                        symbol: "person.2",
                        title: AppLocalization.string("agentManagement.emptyTitle"),
                        detail: AppLocalization.string("agentManagement.emptyHint")
                    )
                } else {
                    ForEach(model.detectedEntries, id: \.id) { entry in
                        AgentRowView(model: model, entry: entry)
                        Divider().padding(.leading, 52)
                    }
                    if !model.undetectedEntries.isEmpty {
                        undetectedFold
                    }
                }
            }
            .padding(.vertical, 6)
        }
    }

    private var undetectedFold: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                model.toggleUndetectedAgents()
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .rotationEffect(.degrees(model.showsUndetectedAgents ? 90 : 0))
                    Text(model.undetectedFoldTitle)
                        .font(.caption.weight(.medium))
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 20)
            .padding(.vertical, 10)
            if model.showsUndetectedAgents {
                ForEach(model.undetectedEntries, id: \.id) { entry in
                    AgentRowView(model: model, entry: entry)
                    Divider().padding(.leading, 52)
                }
            }
        }
        .animation(.easeInOut(duration: 0.18), value: model.showsUndetectedAgents)
    }

    private func messageState(symbol: String, title: String, detail: String? = nil) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(title, systemImage: symbol)
                .font(.callout.weight(.medium))
                .foregroundStyle(.secondary)
            if let detail {
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 24)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var footer: some View {
        HStack(alignment: .center, spacing: 10) {
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
            } else if model.isReloading {
                Text(AppLocalization.string("agentManagement.reloading"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            if let lastCheckedText = model.lastCheckedText {
                Text(lastCheckedText)
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 10)
        .frame(minHeight: 38, alignment: .leading)
    }
}

private struct AgentRowView: View {
    @ObservedObject var model: AgentManagementModel
    let entry: AgentEntry

    var body: some View {
        let presentation = model.rowPresentation(for: entry)
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: agentSymbolName(entry.id))
                    .font(.system(size: 15))
                    .foregroundStyle(.secondary)
                    .frame(width: 22)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 8) {
                        Text(entry.name)
                            .font(.body.weight(.medium))
                        if let cliVersionText = presentation.cliVersionText {
                            Text(cliVersionText)
                                .font(.caption.monospacedDigit())
                                .foregroundStyle(.secondary)
                        } else if entry.cli.isMissing {
                            Text(AppLocalization.string("agentManagement.cliMissing"))
                                .font(.caption)
                                .foregroundStyle(.orange)
                        }
                        if let appVersionText = presentation.appVersionText {
                            Text(appVersionText)
                                .font(.caption.monospacedDigit())
                                .foregroundStyle(.secondary)
                        }
                        if presentation.isUpdating {
                            HStack(spacing: 5) {
                                ProgressView().controlSize(.mini)
                                Text(AppLocalization.string("agentManagement.updating"))
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        } else if let updateText = presentation.updateText {
                            Button {
                                Task { await model.upgrade(agentID: entry.id) }
                            } label: {
                                HStack(spacing: 4) {
                                    Image(systemName: "arrow.down.circle")
                                    Text(updateText)
                                }
                                .font(.caption)
                            }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                            .disabled(model.isBusy)
                            .help(presentation.updateHelp ?? "")
                            .accessibilityLabel(updateText)
                        }
                    }
                    Text(secondaryLineText(presentation))
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .help(secondaryLineHelp(presentation))
                }
                Spacer(minLength: 8)
                Text(presentation.statusText)
                    .font(.caption.weight(.medium))
                    .foregroundStyle(statusColor(presentation))
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2)
                    .background(
                        statusColor(presentation).opacity(0.12),
                        in: Capsule()
                    )
                Toggle("", isOn: connectBinding)
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(model.isBusy)
                    .help(connectHelp(presentation))
                    .accessibilityLabel(connectHelp(presentation))
                Button {
                    model.toggleDetails(agentID: entry.id)
                } label: {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .rotationEffect(.degrees(model.isExpanded(agentID: entry.id) ? 90 : 0))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help(model.isExpanded(agentID: entry.id)
                    ? AppLocalization.string("agentManagement.detailsHide")
                    : AppLocalization.string("agentManagement.details"))
                .accessibilityLabel(AppLocalization.string("agentManagement.details"))
            }
            .padding(.horizontal, 14)
            .frame(height: agentManagementRowHeight)
            .contentShape(Rectangle())
            .onTapGesture {
                model.toggleDetails(agentID: entry.id)
            }
            if model.isExpanded(agentID: entry.id) {
                AgentDetailPanel(entry: entry)
            }
        }
    }

    private var connectBinding: Binding<Bool> {
        Binding(
            get: { entry.integration.state == .managed },
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

    private func secondaryLineText(_ presentation: AgentRowPresentation) -> String {
        if presentation.isUpdating {
            return AppLocalization.string("agentManagement.updating")
        }
        if !presentation.detailText.isEmpty {
            return presentation.detailText
        }
        return entry.cli.path.isEmpty
            ? AppLocalization.string("agentManagement.detail.unknown")
            : entry.cli.path
    }

    private func secondaryLineHelp(_ presentation: AgentRowPresentation) -> String {
        var lines: [String] = []
        if !presentation.detailText.isEmpty {
            lines.append(presentation.detailText)
        }
        if !entry.cli.path.isEmpty {
            lines.append(entry.cli.path)
        }
        if !entry.cli.command.isEmpty {
            lines.append(entry.cli.command)
        }
        return lines.joined(separator: "\n")
    }

    private func statusColor(_ presentation: AgentRowPresentation) -> Color {
        if presentation.statusIsError { return .orange }
        return presentation.isConnected ? .green : .secondary
    }

    private func connectHelp(_ presentation: AgentRowPresentation) -> String {
        if presentation.isConnected {
            return AppLocalization.string("agentManagement.disconnectHelp", entry.name)
        }
        return AppLocalization.string("agentManagement.connectHelp", entry.name)
    }
}

private struct AgentDetailPanel: View {
    let entry: AgentEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            detailRow("agentManagement.detail.version", entry.cli.version)
            detailRow("agentManagement.detail.latest", entry.cli.latest)
            detailRow("agentManagement.detail.via", entry.cli.via)
            detailRow("agentManagement.detail.path", entry.cli.path)
            detailRow("agentManagement.detail.command", entry.cli.command)
            detailRow("agentManagement.detail.appVersion", entry.cli.appVersion)
            detailRow("agentManagement.detail.mode", agentIntegrationModeText(entry.integration.mode))
            detailRow("agentManagement.detail.integration", entry.integration.detail)
            detailRow("agentManagement.detail.error", entry.integration.error)
            detailRow("agentManagement.detail.cliError", entry.cli.error)
            detailRow(
                "agentManagement.detail.disconnectScope",
                agentDisconnectScopeText(entry)
            )
        }
        .padding(.horizontal, 20)
        .padding(.top, 4)
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(nsColor: .controlBackgroundColor).opacity(0.5))
    }

    @ViewBuilder
    private func detailRow(_ key: String, _ value: String?) -> some View {
        if let value, !value.isEmpty {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(AppLocalization.string(key))
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .frame(width: 96, alignment: .leading)
                Text(value)
                    .font(.caption.monospaced())
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }
}

private func agentSymbolName(_ id: String) -> String {
    switch id {
    case "codex": return "chevron.left.forwardslash.chevron.right"
    case "claude": return "text.bubble"
    case "claude-desktop": return "macwindow"
    case "dsh": return "terminal"
    case "opencode": return "curlybraces"
    case "pi": return "function"
    default: return "app"
    }
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
        window.contentViewController = NSHostingController(
            rootView: AgentManagementView(model: model)
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
        Task { @MainActor [weak self] in
            await self?.model.refresh()
        }
    }
}
