import Foundation

enum FusionSettingsError: Error, CustomStringConvertible {
    case message(String)

    var description: String {
        switch self {
        case .message(let message): return message
        }
    }
}

struct FusionModelOption: Hashable {
    let id: String
    let displayName: String
    let isAvailable: Bool

    init(id: String, displayName: String, isAvailable: Bool = true) {
        self.id = id
        self.displayName = displayName
        self.isAvailable = isAvailable
    }
}

struct FusionSettingsProfile {
    var id = "default"
    var panelModels: [String] = []
    var judgeModel = ""
    var finalModel = ""
    var minSuccessful = 1
    var maxCompletionTokens = 2048
    var timeoutMs = 300_000
    var showIntermediateResults = true
    var panelToolsEnabled = true
    var panelMaxRounds = 16
    var panelMaxCallsPerModel = 64

    static func fromCLIJSON(_ rawJSON: String) throws -> FusionSettingsProfile {
        let data = Data(rawJSON.utf8)
        guard
            let envelope = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            throw FusionSettingsError.message("Fusion CLI 返回了无效 JSON")
        }
        guard let profile = envelope["profile"] as? [String: Any] else {
            return FusionSettingsProfile()
        }
        var value = FusionSettingsProfile()
        value.id = profile["id"] as? String ?? value.id
        value.panelModels = profile["panel_models"] as? [String] ?? value.panelModels
        value.judgeModel = profile["judge_model"] as? String ?? value.judgeModel
        value.finalModel = profile["final_model"] as? String ?? value.finalModel
        value.minSuccessful = (profile["min_successful"] as? NSNumber)?.intValue ?? value.minSuccessful
        value.maxCompletionTokens = (profile["max_completion_tokens"] as? NSNumber)?.intValue ?? value.maxCompletionTokens
        value.timeoutMs = (profile["timeout_ms"] as? NSNumber)?.intValue ?? value.timeoutMs
        value.showIntermediateResults = (profile["show_intermediate_results"] as? NSNumber)?.boolValue ?? value.showIntermediateResults
        if let tools = profile["panel_tools"] as? [String: Any] {
            value.panelToolsEnabled = (tools["enabled"] as? NSNumber)?.boolValue ?? value.panelToolsEnabled
            let storedRounds = (tools["max_rounds"] as? NSNumber)?.intValue
            let storedCalls = (tools["max_calls_per_model"] as? NSNumber)?.intValue
            // Automatically migrate the original, overly restrictive defaults.
            value.panelMaxRounds = storedRounds == 4 ? 16 : (storedRounds ?? value.panelMaxRounds)
            value.panelMaxCallsPerModel = storedCalls == 8 ? 64 : (storedCalls ?? value.panelMaxCallsPerModel)
        }
        return value
    }

    var dictionary: [String: Any] {
        [
            "id": id,
            "panel_models": panelModels,
            "judge_model": judgeModel,
            "final_model": finalModel,
            "min_successful": minSuccessful,
            "max_completion_tokens": maxCompletionTokens,
            "timeout_ms": timeoutMs,
            "show_intermediate_results": showIntermediateResults,
            "panel_tools": [
                "enabled": panelToolsEnabled,
                "max_rounds": panelMaxRounds,
                "max_calls_per_model": panelMaxCallsPerModel,
            ],
        ]
    }

    func jsonString() throws -> String {
        let data = try JSONSerialization.data(
            withJSONObject: dictionary,
            options: [.sortedKeys]
        )
        guard let value = String(data: data, encoding: .utf8) else {
            throw FusionSettingsError.message("Fusion 配置无法编码为 UTF-8 JSON")
        }
        return value
    }
}

struct FusionModelSelection: Equatable {
    let panelModels: [String]
    let judgeModel: String
    let finalModel: String
}

func resolveFusionModelSelection(
    availableModelIDs: [String],
    storedPanelModels: [String],
    storedJudgeModel: String,
    storedFinalModel: String
) -> FusionModelSelection {
    let availableModelIDs = availableModelIDs.reduce(into: [String]()) { result, modelID in
        if !result.contains(modelID) {
            result.append(modelID)
        }
    }
    let available = Set(availableModelIDs)
    let storedPanels = storedPanelModels.filter(available.contains).prefix(8)
    let panelModels = storedPanels.isEmpty
        ? Array(availableModelIDs.prefix(3))
        : Array(storedPanels)
    let judgeModel = available.contains(storedJudgeModel)
        ? storedJudgeModel
        : panelModels.first ?? availableModelIDs.first ?? ""
    let finalModel = available.contains(storedFinalModel)
        ? storedFinalModel
        : panelModels.dropFirst().first ?? judgeModel

    return FusionModelSelection(
        panelModels: panelModels,
        judgeModel: judgeModel,
        finalModel: finalModel
    )
}
