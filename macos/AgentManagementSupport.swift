import Foundation

/// Decoded `agent list --json` response. The window renders what the CLI
/// reports; it never decides on its own whether an agent is managed.
struct AgentListResponse: Decodable, Equatable {
    let agents: [AgentEntry]
}

enum AgentIntegrationState: String, Decodable, Equatable {
    case managed
    case unmanaged
    case error
    case unsupported
    case drift
    case pending
    case pendingRemoval = "pending_removal"
}

struct AgentIntegration: Decodable, Equatable {
    let state: AgentIntegrationState
    let mode: String?
    let detail: String
    let error: String?
}

struct AgentCLIInfo: Decodable, Equatable {
    let version: String
    let latest: String
    let via: String
    let path: String
    let command: String
    let appVersion: String?
    let appPath: String?
    let update: Bool
    let error: String?

    private enum CodingKeys: String, CodingKey {
        case version
        case latest
        case via
        case path
        case command
        case appVersion = "app_version"
        case appPath = "app_path"
        case update
        case error
    }

    init(
        version: String,
        latest: String,
        via: String,
        path: String,
        command: String,
        appVersion: String?,
        update: Bool,
        error: String?,
        appPath: String? = nil
    ) {
        self.version = version
        self.latest = latest
        self.via = via
        self.path = path
        self.command = command
        self.appVersion = appVersion
        self.appPath = appPath
        self.update = update
        self.error = error
    }

    /// The CLI serializes the version fields as nullable strings when it
    /// cannot read them, so null and a missing key both mean "unknown" here.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        version = try container.decodeIfPresent(String.self, forKey: .version) ?? ""
        latest = try container.decodeIfPresent(String.self, forKey: .latest) ?? ""
        via = try container.decodeIfPresent(String.self, forKey: .via) ?? ""
        path = try container.decodeIfPresent(String.self, forKey: .path) ?? ""
        command = try container.decodeIfPresent(String.self, forKey: .command) ?? ""
        appVersion = try container.decodeIfPresent(String.self, forKey: .appVersion)
        appPath = try container.decodeIfPresent(String.self, forKey: .appPath)
        update = try container.decode(Bool.self, forKey: .update)
        error = try container.decodeIfPresent(String.self, forKey: .error)
    }

    /// Neither a CLI nor a native desktop installation has been discovered.
    var isMissing: Bool {
        version.isEmpty && path.isEmpty && appVersion == nil && appPath == nil
    }
}

struct AgentEntry: Decodable, Equatable {
    let id: String
    let name: String
    let detected: Bool
    let integration: AgentIntegration
    let cli: AgentCLIInfo

    func replacingCLI(_ cli: AgentCLIInfo) -> AgentEntry {
        AgentEntry(
            id: id,
            name: name,
            detected: detected,
            integration: integration,
            cli: cli
        )
    }
}

struct AgentVersionsResponse: Decodable, Equatable {
    let agents: [AgentVersionsEntry]
}

struct AgentVersionsEntry: Decodable, Equatable {
    let id: String
    let cli: AgentCLIInfo
}

struct AgentUpgradeResponse: Decodable, Equatable {
    let id: String
    let cli: AgentCLIInfo
}

func decodeAgentList(_ json: String) throws -> AgentListResponse {
    do {
        return try JSONDecoder().decode(AgentListResponse.self, from: Data(json.utf8))
    } catch {
        throw GatewayError.command("agent list 返回了无效 JSON：\(error)")
    }
}

func decodeAgentVersions(_ json: String) throws -> AgentVersionsResponse {
    do {
        return try JSONDecoder().decode(AgentVersionsResponse.self, from: Data(json.utf8))
    } catch {
        throw GatewayError.command("agent versions 返回了无效 JSON：\(error)")
    }
}

func decodeAgentUpgrade(_ json: String) throws -> AgentUpgradeResponse {
    do {
        return try JSONDecoder().decode(AgentUpgradeResponse.self, from: Data(json.utf8))
    } catch {
        throw GatewayError.command("agent upgrade 返回了无效 JSON：\(error)")
    }
}

// MARK: - Ordering and selection

enum AgentCatalog {
    /// Display order of the agents this product ships today. Agents the CLI
    /// adds later keep their place after these, ordered by name.
    static let knownOrder = ["codex", "claude", "claude-desktop", "dsh", "opencode", "pi"]

    static func rank(forID id: String) -> Int {
        knownOrder.firstIndex(of: id) ?? knownOrder.count
    }
}

func orderedAgentEntries(_ agents: [AgentEntry]) -> [AgentEntry] {
    agents.sorted { left, right in
        let leftRank = AgentCatalog.rank(forID: left.id)
        let rightRank = AgentCatalog.rank(forID: right.id)
        if leftRank != rightRank {
            return leftRank < rightRank
        }
        let nameOrder = left.name.localizedStandardCompare(right.name)
        if nameOrder != .orderedSame {
            return nameOrder == .orderedAscending
        }
        return left.id < right.id
    }
}

// A version check that finds the CLI outranks the earlier list result.
func detectedAgentEntries(_ agents: [AgentEntry]) -> [AgentEntry] {
    orderedAgentEntries(agents.filter { $0.detected || !$0.cli.isMissing })
}

func undetectedAgentEntries(_ agents: [AgentEntry]) -> [AgentEntry] {
    orderedAgentEntries(agents.filter { !$0.detected && $0.cli.isMissing })
}

func upgradableAgentEntries(_ agents: [AgentEntry]) -> [AgentEntry] {
    detectedAgentEntries(agents).filter { $0.cli.update }
}

// MARK: - CLI command contract

enum AgentConnectMode: String, CaseIterable, Equatable {
    case officialAccount = "official-account"
    case customOnly = "custom-only"

    /// Short label for the connection detail panel.
    var shortLabel: String {
        switch self {
        case .officialAccount: return AppLocalization.string("agentManagement.mode.officialAccount")
        case .customOnly: return AppLocalization.string("agentManagement.mode.customOnly")
        }
    }

    /// Radio title in the Codex connect panel.
    var pickerTitle: String {
        switch self {
        case .officialAccount: return AppLocalization.string("codexConnect.modeOfficial")
        case .customOnly: return AppLocalization.string("codexConnect.modeCustomOnly")
        }
    }

    var pickerDetail: String {
        switch self {
        case .officialAccount: return AppLocalization.string("codexConnect.officialDetail")
        case .customOnly: return AppLocalization.string("codexConnect.customOnlyDetail")
        }
    }

    var providerDescription: String {
        switch self {
        case .officialAccount: return AppLocalization.string("codexConnect.officialProvider")
        case .customOnly: return AppLocalization.string("codexConnect.customOnlyProvider")
        }
    }
}

/// Only Codex takes a connection mode; every other agent connects with one
/// command.
func agentRequiresConnectMode(id: String) -> Bool {
    id == "codex"
}

func agentListArguments() -> [String] {
    ["agent", "list", "--json"]
}

func agentVersionsArguments() -> [String] {
    ["agent", "versions", "--json", "--refresh"]
}

func agentUpgradeArguments(id: String) -> [String] {
    ["agent", "upgrade", id, "--json"]
}

func agentConnectArguments(id: String, mode: AgentConnectMode?) -> [String] {
    var arguments = ["agent", "connect", id]
    if let mode, agentRequiresConnectMode(id: id) {
        arguments.append(contentsOf: ["--mode", mode.rawValue])
    }
    return arguments
}

/// Disconnecting one agent only touches that agent: the shared local gateway
/// keeps serving every other agent, so this never stops the service.
func agentDisconnectArguments(id: String) -> [String] {
    ["agent", "disconnect", id]
}

// MARK: - Row presentation

struct AgentRowPresentation: Equatable {
    let name: String
    let cliVersionText: String?
    let appVersionText: String?
    let updateText: String?
    let updateHelp: String?
    let isUpdating: Bool
    let statusText: String
    let statusIsError: Bool
    let detailText: String
    let isConnected: Bool
    let isBusy: Bool
}

func agentIntegrationStateText(_ state: AgentIntegrationState) -> String {
    switch state {
    case .managed: return AppLocalization.string("agentManagement.state.managed")
    case .unmanaged: return AppLocalization.string("agentManagement.state.unmanaged")
    case .error: return AppLocalization.string("agentManagement.state.error")
    case .unsupported: return "Unsupported"
    case .drift: return "Configuration drift"
    case .pending: return AppLocalization.string("agentManagement.state.pending")
    case .pendingRemoval: return AppLocalization.string("agentManagement.state.pendingRemoval")
    }
}

/// The CLI reports Codex's mode as `codex_oauth_proxy` / `custom_only`; the
/// connect command takes `official-account` / `custom-only`. Anything else is
/// shown as the CLI wrote it.
func agentIntegrationModeText(_ mode: String?) -> String? {
    guard let mode, !mode.isEmpty else { return nil }
    switch mode {
    case "codex_oauth_proxy", AgentConnectMode.officialAccount.rawValue:
        return AgentConnectMode.officialAccount.shortLabel
    case "custom_only", AgentConnectMode.customOnly.rawValue:
        return AgentConnectMode.customOnly.shortLabel
    case "native_models": return AppLocalization.string("agentManagement.mode.nativeModels")
    case "native_credentials": return AppLocalization.string("agentManagement.mode.nativeCredentials")
    case "native_runtime": return AppLocalization.string("agentManagement.mode.nativeRuntime")
    case "native_app": return AppLocalization.string("agentManagement.mode.nativeApp")
    case "confirmed_import": return AppLocalization.string("agentManagement.mode.confirmedImport")
    default:
        return mode
    }
}

func agentRowPresentation(for entry: AgentEntry, isBusy: Bool) -> AgentRowPresentation {
    var parts: [String] = []
    if let mode = agentIntegrationModeText(entry.integration.mode) {
        parts.append(mode)
    }
    if entry.integration.state == .pending || entry.integration.state == .pendingRemoval {
        parts.append(entry.integration.detail)
    }
    // integration.detail describes what disconnect restores; it belongs in
    // the details panel and the confirmation, not the one-line summary.
    if let integrationError = entry.integration.error, !integrationError.isEmpty {
        parts.append(integrationError)
    }
    if let cliError = entry.cli.error, !cliError.isEmpty {
        parts.append(AppLocalization.string("agentManagement.cliErrorPrefix", cliError))
    }
    let statusText = entry.integration.mode == "native_models" && entry.integration.state == .managed
        ? AppLocalization.string("agentManagement.state.nativeManaged")
        : agentIntegrationStateText(entry.integration.state)
    let updateText = entry.cli.update && !entry.cli.latest.isEmpty
        ? AppLocalization.string("agentManagement.updateTo", entry.cli.latest)
        : nil
    return AgentRowPresentation(
        name: entry.name,
        // "CLI" only tells the two apart when an app version is shown too.
        cliVersionText: entry.cli.version.isEmpty
            ? nil
            : entry.cli.appVersion == nil
            ? entry.cli.version
            : AppLocalization.string("agentManagement.cliVersion", entry.cli.version),
        appVersionText: entry.cli.appVersion.map {
            AppLocalization.string("agentManagement.appVersion", $0)
        },
        updateText: updateText,
        updateHelp: updateText == nil
            ? nil
            : AppLocalization.string("agentManagement.updateHelp", entry.cli.command),
        isUpdating: isBusy,
        statusText: statusText,
        statusIsError: entry.integration.state == .error,
        detailText: parts.joined(separator: AppLocalization.string("agentManagement.detailSeparator")),
        isConnected: [.managed, .drift, .pending, .pendingRemoval].contains(entry.integration.state),
        isBusy: isBusy
    )
}

func agentUndetectedFoldTitle(count: Int) -> String {
    AppLocalization.string("agentManagement.undetectedFold", String(count))
}

func agentDisconnectScopeText(_ entry: AgentEntry) -> String {
    let detail = entry.integration.detail.trimmingCharacters(in: .whitespacesAndNewlines)
    guard detail.isEmpty else { return detail }
    return AppLocalization.string("agentManagement.detail.disconnectScopeFallback")
}

/// Confirmation shown before an agent is disconnected. The restore scope
/// comes from the CLI's own integration detail so the app never invents what
/// gets restored.
func agentDisconnectConfirmation(for entry: AgentEntry) -> (title: String, message: String) {
    (
        title: AppLocalization.string("agentManagement.disconnectTitle", entry.name),
        message: AppLocalization.string(
            "agentManagement.disconnectMessage",
            entry.name,
            agentDisconnectScopeText(entry),
            AppLocalization.string("agentManagement.disconnectGatewayNote")
        )
    )
}

func agentLastCheckedText(_ date: Date) -> String {
    AppLocalization.string(
        "agentManagement.lastChecked",
        DateFormatter.localizedString(from: date, dateStyle: .none, timeStyle: .medium)
    )
}
