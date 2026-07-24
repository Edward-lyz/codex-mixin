import Foundation

struct ProviderListResponse: Decodable {
    let configVersion: UInt64
    let gatewayBind: String?
    let gatewayAuthConfigured: Bool
    let providers: [ProviderView]

    enum CodingKeys: String, CodingKey {
        case configVersion = "config_version"
        case gatewayBind = "gateway_bind"
        case gatewayAuthConfigured = "gateway_auth_configured"
        case providers
    }
}

struct ProviderView: Decodable {
    let id: String
    let displayName: String
    let enabled: Bool
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
                    contextWindow: nil
                ),
                isAvailable: false,
                isNew: false
            )
        }
    }
}

struct ProviderModelSourceView: Decodable {
    let kind: String
    let path: String?
}

struct ProviderModelView: Decodable {
    let id: String
    let displayName: String?
    let description: String?
    let contextWindow: UInt64?

    enum CodingKeys: String, CodingKey {
        case id
        case displayName = "display_name"
        case description
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
    var contextWindow: UInt64? { model.contextWindow }
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
