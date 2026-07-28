import Foundation

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

enum AuxiliaryModelSupport: Equatable {
    case none
    case autoReviewOnly
    case voiceOnly
    case autoReviewAndVoice
}

struct ProviderView: Decodable {
    let id: String
    let displayName: String
    let enabled: Bool
    let auxiliaryModelUpstream: Bool
    let presetID: String?
    let protocolID: String
    let baseURL: String
    let apiPath: String
    let modelSource: ProviderModelSourceView
    let apiKeyConfigured: Bool
    let imageGenerationPath: String?
    let quotaURL: String?
    let quotaUsername: String?
    let quotaCurrency: String?
    let quotaParser: String
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
        case displayName = "display_name"
        case enabled
        case auxiliaryModelUpstream = "auxiliary_model_upstream"
        case presetID = "preset_id"
        case protocolID = "protocol"
        case baseURL = "base_url"
        case apiPath = "api_path"
        case modelSource = "model_source"
        case apiKeyConfigured = "api_key_configured"
        case imageGenerationPath = "image_generation_path"
        case quotaURL = "quota_url"
        case quotaUsername = "quota_username"
        case quotaCurrency = "quota_currency"
        case quotaParser = "quota_parser"
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

    var modelsPath: String? {
        modelSource.path
    }

    var supportsAutoReview: Bool {
        cachedModels.contains { $0.id == "codex-auto-review" }
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
        } + unavailableSelectedModels.map {
            ProviderModelListItem(
                model: ProviderModelView(
                    id: $0,
                    displayName: nil,
                    description: "该模型仍保留在 allowlist，但本次模型发现未返回它。",
                    ratio: nil,
                    priceType: nil,
                    contextWindow: nil
                ),
                isAvailable: false,
                isNew: false
            )
        }
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
    let displayName: String?
    let description: String?
    let ratio: String?
    let priceType: String?
    let contextWindow: UInt64?

    enum CodingKeys: String, CodingKey {
        case id
        case displayName = "display_name"
        case description
        case ratio
        case priceType = "price_type"
        case contextWindow = "context_window"
    }
}

struct ProviderModelListItem {
    let model: ProviderModelView
    let isAvailable: Bool
    let isNew: Bool

    var id: String { model.id }
    var displayName: String? { model.displayName }
    var description: String? { model.description }
    var ratio: String? { model.ratio }
    var priceType: String? { model.priceType }
    var contextWindow: UInt64? { model.contextWindow }
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
        .init(id: "selected", title: "加入 Codex", width: 94, minimumWidth: 86, defaultAscending: false),
        .init(id: "model", title: "上游模型", width: 520, minimumWidth: 260, defaultAscending: true),
        .init(id: "ttft", title: "TTFT", width: 104, minimumWidth: 82, defaultAscending: true),
        .init(id: "tps", title: "吞吐", width: 112, minimumWidth: 88, defaultAscending: false),
        .init(id: "context", title: "上下文", width: 104, minimumWidth: 84, defaultAscending: false),
        .init(id: "ratio", title: "倍率", width: 86, minimumWidth: 70, defaultAscending: true),
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

func configuredProviderOptions(_ providers: [ProviderView]) -> [ProviderPickerOption] {
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
    selectedKeys: Set<String>
) -> [String: [String]] {
    Dictionary(uniqueKeysWithValues: providers.map { provider in
        let modelIDs = provider.modelItems
            .filter {
                selectedKeys.contains(
                    providerModelSelectionKey(providerID: provider.id, modelID: $0.id)
                )
            }
            .map(\.id)
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
