import Foundation

private let manuallyEnteredModelContextWindow: UInt64 = 128_000
private let modelContextTokensPerK: UInt64 = 1_000

func modelContextK(fromTokens tokens: UInt64) -> UInt64 {
    max(tokens / modelContextTokensPerK, 1)
}

func modelContextTokens(fromK contextK: UInt64) -> UInt64 {
    min(max(contextK, 1), UInt64.max / modelContextTokensPerK) * modelContextTokensPerK
}

func modelCapabilityProbeCounts(_ progress: String) -> (completed: Int, total: Int)? {
    guard
        progress.hasPrefix("Probing capabilities for "),
        let counterStart = progress.range(of: ": ", options: .backwards)?.upperBound,
        let counterEnd = progress.range(of: " complete", range: counterStart..<progress.endIndex)?
            .lowerBound
    else {
        return nil
    }
    let counts = progress[counterStart..<counterEnd].split(separator: "/", maxSplits: 1)
    guard
        counts.count == 2,
        let completed = Int(counts[0]),
        let total = Int(counts[1]),
        completed >= 0,
        total > 0,
        completed <= total
    else {
        return nil
    }
    return (completed, total)
}

private func managedPackageVersion(
    at versionFile: URL,
    fileManager: FileManager
) -> String? {
    guard fileManager.isReadableFile(atPath: versionFile.path),
          let contents = try? String(contentsOf: versionFile, encoding: .utf8)
    else {
        return nil
    }
    let version = contents.trimmingCharacters(in: .whitespacesAndNewlines)
    let components = version.split(
        separator: ".",
        omittingEmptySubsequences: false
    )
    guard components.count >= 3,
          components.allSatisfy({
              !$0.isEmpty && $0.allSatisfy(\.isNumber)
          })
    else {
        return nil
    }
    return version
}

func isManagedVersion(_ candidate: String, newerThan installed: String) -> Bool {
    guard let candidateParts = managedVersionComponents(candidate),
          let installedParts = managedVersionComponents(installed)
    else {
        return false
    }
    let count = max(candidateParts.count, installedParts.count)
    for index in 0..<count {
        let candidatePart = index < candidateParts.count ? candidateParts[index] : 0
        let installedPart = index < installedParts.count ? installedParts[index] : 0
        if candidatePart != installedPart {
            return candidatePart > installedPart
        }
    }
    return false
}

private func managedVersionComponents(_ version: String) -> [UInt64]? {
    let parts = version.split(separator: ".", omittingEmptySubsequences: false)
    guard parts.count >= 3 else { return nil }
    let components = parts.compactMap { UInt64($0) }
    return components.count == parts.count ? components : nil
}

enum BaiduAuthBridgeMode: String, Decodable, Equatable {
    case disabled
    case ducxLoopback = "ducx_loopback"
}

func baiduBridgeNeedsSetup(
    current: BaiduAuthBridgeMode?,
    selected: BaiduAuthBridgeMode
) -> Bool {
    selected != .disabled && current != selected
}

enum ManagedCodexInstallMode: String, Decodable {
    case customOnly = "custom_only"
    case codexOAuthProxy = "codex_oauth_proxy"
}

struct ProviderListResponse: Decodable {
    let configVersion: UInt64
    let gatewayBind: String?
    let gatewayAuthConfigured: Bool
    let codexInstallMode: ManagedCodexInstallMode?
    let providers: [ProviderView]

    enum CodingKeys: String, CodingKey {
        case configVersion = "config_version"
        case gatewayBind = "gateway_bind"
        case gatewayAuthConfigured = "gateway_auth_configured"
        case codexInstallMode = "codex_install_mode"
        case providers
    }
}

struct GatewayStatusProviderReadiness: Decodable {
    let status: String
    let routableModelCount: Int
    let selectedModelCount: Int
    let availableModelCount: Int
    let unavailableSelectedModelCount: Int
    let lastModelRefreshError: String?
    let issues: [String]

    enum CodingKeys: String, CodingKey {
        case status
        case routableModelCount = "routable_model_count"
        case selectedModelCount = "selected_model_count"
        case availableModelCount = "available_model_count"
        case unavailableSelectedModelCount = "unavailable_selected_model_count"
        case lastModelRefreshError = "last_model_refresh_error"
        case issues
    }
}

struct GatewayStatusProvider: Decodable {
    let id: String
    let displayName: String
    let enabled: Bool
    let readiness: GatewayStatusProviderReadiness

    enum CodingKeys: String, CodingKey {
        case id
        case displayName = "display_name"
        case enabled
        case readiness
    }
}

struct GatewayStatusSnapshot: Decodable {
    let configured: Bool?
    let gateway: String?
    let endpoint: String?
    let providerReadiness: String?
    let providers: [GatewayStatusProvider]?

    enum CodingKeys: String, CodingKey {
        case configured
        case gateway
        case endpoint
        case providerReadiness = "provider_readiness"
        case providers
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        configured = try values.decodeIfPresent(Bool.self, forKey: .configured)
        gateway = try values.decodeIfPresent(String.self, forKey: .gateway)
        endpoint = try values.decodeIfPresent(String.self, forKey: .endpoint)
        providerReadiness = try values.decodeIfPresent(String.self, forKey: .providerReadiness)
        if gateway == "running" {
            providers = try values.decode([GatewayStatusProvider].self, forKey: .providers)
        } else {
            providers = try? values.decode([GatewayStatusProvider].self, forKey: .providers)
        }
    }
}

enum ProviderKind: String, Decodable {
    case configured
    case official
}

enum AuxiliaryModelSupport: Equatable {
    case none
    case autoReviewOnly
    case voiceOnly
    case autoReviewAndVoice
}

struct ProviderView: Decodable {
    let id: String
    let kind: ProviderKind
    let displayName: String
    let enabled: Bool
    let auxiliaryModelUpstream: Bool
    let autoReviewModel: String?
    let presetID: String?
    let protocolID: String
    let baseURL: String
    let websiteURL: String?
    let apiPath: String
    let modelSource: ProviderModelSourceView
    let apiKeyConfigured: Bool
    let awsSigV4Configured: Bool?
    let awsRegion: String?
    let awsSessionTokenConfigured: Bool?
    let imageGenerationPath: String?
    let quotaURL: String?
    let quotaUsername: String?
    let quotaWorkspaceID: String?
    let quotaAuthCookieConfigured: Bool?
    let quotaCurrency: String?
    let quotaParser: String
    let baiduAuthBridge: BaiduAuthBridgeMode?
    let baiduCodeReport: Bool?
    let selectedModels: [String]
    let newModels: [String]
    let unavailableSelectedModels: [String]
    let cachedModels: [ProviderModelView]
    let modelsRefreshedAtMilliseconds: UInt64?
    let lastModelRefreshError: String?
    let readiness: String
    let readinessIssues: [String]
    let routableModelCount: Int

    enum CodingKeys: String, CodingKey {
        case id
        case kind
        case displayName = "display_name"
        case enabled
        case auxiliaryModelUpstream = "auxiliary_model_upstream"
        case autoReviewModel = "auto_review_model"
        case presetID = "preset_id"
        case protocolID = "protocol"
        case baseURL = "base_url"
        case websiteURL = "website_url"
        case apiPath = "api_path"
        case modelSource = "model_source"
        case apiKeyConfigured = "api_key_configured"
        case awsSigV4Configured = "aws_sigv4_configured"
        case awsRegion = "aws_region"
        case awsSessionTokenConfigured = "aws_session_token_configured"
        case imageGenerationPath = "image_generation_path"
        case quotaURL = "quota_url"
        case quotaUsername = "quota_username"
        case quotaWorkspaceID = "quota_workspace_id"
        case quotaAuthCookieConfigured = "quota_auth_cookie_configured"
        case quotaCurrency = "quota_currency"
        case quotaParser = "quota_parser"
        case baiduAuthBridge = "baidu_auth_bridge"
        case baiduCodeReport = "baidu_code_report"
        case selectedModels = "selected_models"
        case newModels = "new_models"
        case unavailableSelectedModels = "unavailable_selected_models"
        case cachedModels = "cached_models"
        case modelsRefreshedAtMilliseconds = "models_refreshed_at_ms"
        case lastModelRefreshError = "last_model_refresh_error"
        case readiness
        case readinessIssues = "readiness_issues"
        case routableModelCount = "routable_model_count"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        kind = try values.decodeIfPresent(ProviderKind.self, forKey: .kind) ?? .configured
        displayName = try values.decode(String.self, forKey: .displayName)
        enabled = try values.decode(Bool.self, forKey: .enabled)
        auxiliaryModelUpstream = try values.decode(Bool.self, forKey: .auxiliaryModelUpstream)
        autoReviewModel = try values.decodeIfPresent(String.self, forKey: .autoReviewModel)
        presetID = try values.decodeIfPresent(String.self, forKey: .presetID)
        protocolID = try values.decode(String.self, forKey: .protocolID)
        baseURL = try values.decode(String.self, forKey: .baseURL)
        websiteURL = try values.decodeIfPresent(String.self, forKey: .websiteURL)
        apiPath = try values.decode(String.self, forKey: .apiPath)
        modelSource = try values.decode(ProviderModelSourceView.self, forKey: .modelSource)
        apiKeyConfigured = try values.decode(Bool.self, forKey: .apiKeyConfigured)
        awsSigV4Configured = try values.decodeIfPresent(Bool.self, forKey: .awsSigV4Configured)
        awsRegion = try values.decodeIfPresent(String.self, forKey: .awsRegion)
        awsSessionTokenConfigured = try values.decodeIfPresent(
            Bool.self,
            forKey: .awsSessionTokenConfigured
        )
        imageGenerationPath = try values.decodeIfPresent(String.self, forKey: .imageGenerationPath)
        quotaURL = try values.decodeIfPresent(String.self, forKey: .quotaURL)
        quotaUsername = try values.decodeIfPresent(String.self, forKey: .quotaUsername)
        quotaWorkspaceID = try values.decodeIfPresent(String.self, forKey: .quotaWorkspaceID)
        quotaAuthCookieConfigured = try values.decodeIfPresent(Bool.self, forKey: .quotaAuthCookieConfigured)
        quotaCurrency = try values.decodeIfPresent(String.self, forKey: .quotaCurrency)
        quotaParser = try values.decode(String.self, forKey: .quotaParser)
        baiduAuthBridge = try values.decodeIfPresent(BaiduAuthBridgeMode.self, forKey: .baiduAuthBridge)
        baiduCodeReport = try values.decodeIfPresent(Bool.self, forKey: .baiduCodeReport)
        selectedModels = try values.decode([String].self, forKey: .selectedModels)
        newModels = try values.decode([String].self, forKey: .newModels)
        unavailableSelectedModels = try values.decode([String].self, forKey: .unavailableSelectedModels)
        cachedModels = try values.decode([ProviderModelView].self, forKey: .cachedModels)
        modelsRefreshedAtMilliseconds = try values.decodeIfPresent(UInt64.self, forKey: .modelsRefreshedAtMilliseconds)
        lastModelRefreshError = try values.decodeIfPresent(String.self, forKey: .lastModelRefreshError)
        readiness = try values.decode(String.self, forKey: .readiness)
        readinessIssues = try values.decode([String].self, forKey: .readinessIssues)
        routableModelCount = try values.decode(Int.self, forKey: .routableModelCount)
    }

    var effectiveBaiduAuthBridge: BaiduAuthBridgeMode? {
        baiduAuthBridge
    }

    var modelsPath: String? {
        modelSource.path
    }

    var supportsAutoReview: Bool {
        if modelSource.kind == "baidu_oneapi" {
            return true
        }
        // Any added model can answer auto review through an explicit choice;
        // an upstream review model maps automatically.
        if !selectedModels.isEmpty {
            return true
        }
        return cachedModels.contains { model in
            switch model.id.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
            case "codex-auto-review", "auto", "auto-baidu-oneapi":
                true
            default:
                false
            }
        }
    }

    var supportsVoice: Bool {
        let voiceModelIDs = Set([
            "gpt-realtime-1.5",
            "gpt-live-1-boulder-alpha",
        ])
        return cachedModels.contains { voiceModelIDs.contains($0.id) }
    }

    var auxiliaryModelSupport: AuxiliaryModelSupport {
        switch (supportsAutoReview, supportsVoice) {
        case (false, false):
            return .none
        case (true, false):
            return .autoReviewOnly
        case (false, true):
            return .voiceOnly
        case (true, true):
            return .autoReviewAndVoice
        }
    }

    var modelItems: [ProviderModelListItem] {
        let newModelIDs = Set(newModels)
        return cachedModels.map {
            ProviderModelListItem(model: $0, isAvailable: true, isNew: newModelIDs.contains($0.id))
        }
    }

    var needsInitialModelDiscovery: Bool {
        kind == .configured
            && enabled
            && modelsRefreshedAtMilliseconds == nil
            && cachedModels.isEmpty
    }

    var supportsModelRefresh: Bool {
        kind == .configured || kind == .official
    }
}

func isAuxiliaryModelUpstreamSelectable(
    for provider: ProviderView,
    codexInstallMode: ManagedCodexInstallMode?
) -> Bool {
    codexInstallMode != .customOnly || provider.auxiliaryModelSupport != .none
}

struct ProviderModelSourceView: Decodable {
    let kind: String
    let path: String?
}

struct ProviderModelView: Decodable {
    let id: String
    let manuallyAdded: Bool?
    let displayName: String?
    let description: String?
    let ratio: String?
    let priceType: String?
    let contextWindow: UInt64?
    let protocolID: String?
    let supportsImage: Bool?
    let supportsThinking: Bool?
    let supportsWebSearch: Bool?
    let supportsToolSearch: Bool?
    let supportsFunctionTools: Bool?
    let capabilityProbeError: String?

    enum CodingKeys: String, CodingKey {
        case id
        case manuallyAdded = "manually_added"
        case displayName = "display_name"
        case description
        case ratio
        case priceType = "price_type"
        case contextWindow = "context_window"
        case protocolID = "protocol"
        case supportsImage = "supports_image"
        case supportsThinking = "supports_thinking"
        case supportsWebSearch = "supports_web_search"
        case supportsToolSearch = "supports_tool_search"
        case supportsFunctionTools = "supports_function_tools"
        case capabilityProbeError = "capability_probe_error"
    }
}

struct ProviderModelListItem {
    let model: ProviderModelView
    let isAvailable: Bool
    let isNew: Bool

    var id: String { model.id }
    var manuallyAdded: Bool { model.manuallyAdded == true }
    var displayName: String? { model.displayName }
    var description: String? { model.description }
    var ratio: String? { model.ratio }
    var priceType: String? { model.priceType }
    var contextWindow: UInt64? { model.contextWindow }
    var protocolID: String? { model.protocolID }
    var supportsImage: Bool? { model.supportsImage }
    var supportsThinking: Bool? { model.supportsThinking }
    var supportsWebSearch: Bool? { model.supportsWebSearch }
    var supportsToolSearch: Bool? { model.supportsToolSearch }
    var supportsFunctionTools: Bool? { model.supportsFunctionTools }
    var capabilityProbeError: String? { model.capabilityProbeError }
}

func manuallyEnteredProviderModel(_ id: String) -> ProviderModelListItem {
    ProviderModelListItem(
        model: ProviderModelView(
            id: id,
            manuallyAdded: true,
            displayName: nil,
            description: "用户手动指定；能力将由后台自动补齐",
            ratio: nil,
            priceType: nil,
            contextWindow: manuallyEnteredModelContextWindow,
            protocolID: nil,
            supportsImage: nil,
            supportsThinking: nil,
            supportsWebSearch: nil,
            supportsToolSearch: nil,
            supportsFunctionTools: nil,
            capabilityProbeError: nil
        ),
        isAvailable: true,
        isNew: false
    )
}

func unavailableProviderModel(_ id: String) -> ProviderModelListItem {
    ProviderModelListItem(
        model: ProviderModelView(
            id: id,
            manuallyAdded: false,
            displayName: nil,
            description: "该已选模型不在当前模型列表中",
            ratio: nil,
            priceType: nil,
            contextWindow: nil,
            protocolID: nil,
            supportsImage: nil,
            supportsThinking: nil,
            supportsWebSearch: nil,
            supportsToolSearch: nil,
            supportsFunctionTools: nil,
            capabilityProbeError: nil
        ),
        isAvailable: false,
        isNew: false
    )
}

func mergedProviderModelItems(
    _ provider: ProviderView,
    additionalModelIDs: Set<String>,
    excludingModelIDs: Set<String>
) -> [ProviderModelListItem] {
    let additional = additionalModelIDs.sorted().map(manuallyEnteredProviderModel)
    let unavailable = provider.unavailableSelectedModels.sorted().map(unavailableProviderModel)
    var seenModelIDs = Set<String>()
    return (provider.modelItems + unavailable + additional).filter { model in
        !excludingModelIDs.contains(model.id) && seenModelIDs.insert(model.id).inserted
    }
}

struct ProviderPickerOption {
    let id: String
    let displayName: String
}

struct ModelBenchmarkColumnDefinition: Equatable {
    let id: String
    let title: String
    let width: Double
    let minimumWidth: Double
    let defaultAscending: Bool
}

func modelBenchmarkColumnDefinitions() -> [ModelBenchmarkColumnDefinition] {
    [
        .init(id: "selected", title: "Codex", width: 64, minimumWidth: 56, defaultAscending: false),
        .init(id: "model", title: "模型", width: 520, minimumWidth: 260, defaultAscending: true),
        .init(id: "ttft", title: "首 Token", width: 104, minimumWidth: 84, defaultAscending: true),
        .init(id: "tps", title: "生成速度", width: 112, minimumWidth: 92, defaultAscending: false),
        .init(id: "context", title: "上下文", width: 104, minimumWidth: 84, defaultAscending: false),
        .init(id: "ratio", title: "倍率", width: 86, minimumWidth: 70, defaultAscending: true),
        .init(id: "capabilities", title: "能力", width: 170, minimumWidth: 150, defaultAscending: false),
    ]
}

func benchmarkRatioValue(_ ratio: String?) -> Double? {
    guard let ratio else { return nil }
    return Double(
        ratio
            .lowercased()
            .replacingOccurrences(of: "x", with: "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
    )
}

func modelSelectionProviderOptions(_ providers: [ProviderView]) -> [ProviderPickerOption] {
    providers.map {
        ProviderPickerOption(id: $0.id, displayName: $0.displayName)
    }
}

func shouldShowModelRatioColumn(for provider: ProviderView?) -> Bool {
    provider?.presetID == "baidu-oneapi"
}

func formatContextWindow(_ value: UInt64) -> String {
    if value >= 1_000_000 {
        return String(format: "%.1fM", Double(value) / 1_000_000)
    }
    if value >= 1_000 {
        return String(format: "%.0fK", Double(value) / 1_000)
    }
    return "\(value)"
}

func providerModelSelectionKey(providerID: String, modelID: String) -> String {
    "\(providerID)\u{1f}\(modelID)"
}

func selectedProviderModelKeys(_ providers: [ProviderView]) -> Set<String> {
    Set(providers.flatMap { provider in
        provider.selectedModels.map {
            providerModelSelectionKey(providerID: provider.id, modelID: $0)
        }
    })
}

func providerModelSelections(
    _ providers: [ProviderView],
    selectedKeys: Set<String>,
    additionalModelIDs: [String: Set<String>] = [:]
) -> [String: [String]] {
    Dictionary(uniqueKeysWithValues: providers.map { provider in
        let modelIDs = (
            provider.modelItems.map(\.id)
                + provider.unavailableSelectedModels
                + Array(additionalModelIDs[provider.id] ?? [])
        )
            .filter { modelID in
                selectedKeys.contains(
                    providerModelSelectionKey(providerID: provider.id, modelID: modelID)
                )
            }
            .reduce(into: [String]()) { modelIDs, modelID in
                if !modelIDs.contains(modelID) { modelIDs.append(modelID) }
            }
        return (provider.id, modelIDs)
    })
}

struct ProviderTestResponse: Decodable {
    let providerID: String
    let ok: Bool
    let mode: String
    let modelCount: Int
    let paidInferencePerformed: Bool

    enum CodingKeys: String, CodingKey {
        case providerID = "provider_id"
        case ok
        case mode
        case modelCount = "model_count"
        case paidInferencePerformed = "paid_inference_performed"
    }
}

func decodeProviderList(_ json: String) throws -> ProviderListResponse {
    do {
        return try JSONDecoder().decode(ProviderListResponse.self, from: Data(json.utf8))
    } catch {
        throw GatewayError.command("供应商列表 JSON 无法解析：\(error)")
    }
}

func decodeGatewayStatus(_ json: String) throws -> GatewayStatusSnapshot {
    do {
        return try JSONDecoder().decode(GatewayStatusSnapshot.self, from: Data(json.utf8))
    } catch {
        throw GatewayError.command("网关状态 JSON 无法解析：\(error)")
    }
}

func gatewayProviderIssueDetails(_ providers: [GatewayStatusProvider]) -> [String] {
    providers.flatMap { provider -> [String] in
        guard provider.enabled, provider.readiness.status == "degraded" else { return [] }
        let readiness = provider.readiness
        var details: [String] = []
        if readiness.issues.contains("credentials_missing") {
            details.append("\(provider.displayName)：尚未配置密钥")
        }
        if readiness.issues.contains("no_routable_models") {
            if readiness.selectedModelCount == 0, readiness.availableModelCount > 0 {
                details.append("\(provider.displayName)：还没有加入模型")
            } else if readiness.availableModelCount == 0 {
                details.append("\(provider.displayName)：尚未获取模型列表")
            } else {
                details.append("\(provider.displayName)：当前已选模型暂不可用")
            }
        }
        if readiness.issues.contains("selected_models_unavailable"),
           readiness.unavailableSelectedModelCount > 0,
           readiness.routableModelCount > 0 {
            details.append(
                "\(provider.displayName)：\(readiness.unavailableSelectedModelCount) 个已选模型不在当前列表，其余 \(readiness.routableModelCount) 个可用"
            )
        }
        if readiness.issues.contains("model_refresh_failed") {
            let error = readiness.lastModelRefreshError?
                .split(whereSeparator: \.isNewline)
                .joined(separator: " ")
            let suffix = error.flatMap { $0.isEmpty ? nil : "：\($0)" } ?? ""
            details.append("\(provider.displayName)：模型列表更新失败\(suffix)")
        }
        return details.isEmpty ? ["\(provider.displayName)：配置需要处理"] : details
    }
}

func providerIssueDetails(fromGatewayStatus status: String?) -> [String] {
    let prefix = "provider-issue: "
    return status?
        .split(separator: "\n")
        .compactMap { line in
            guard line.hasPrefix(prefix) else { return nil }
            let detail = String(line.dropFirst(prefix.count))
            return detail.isEmpty ? nil : detail
        } ?? []
}

func decodeProviderTest(_ json: String) throws -> ProviderTestResponse {
    do {
        return try JSONDecoder().decode(ProviderTestResponse.self, from: Data(json.utf8))
    } catch {
        throw GatewayError.command("供应商测试 JSON 无法解析：\(error)")
    }
}

func appendProviderArgument(_ arguments: inout [String], _ name: String, _ rawValue: String) {
    let value = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
    if !value.isEmpty {
        arguments.append(name)
        arguments.append(value)
    }
}
