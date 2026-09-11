import Foundation

enum FusionSettingsError: Error, CustomStringConvertible {
    case message(String)

    var description: String {
        switch self {
        case .message(let message): return message
        }
    }
}

enum FusionSettingsMode: String, CaseIterable, Identifiable {
    case orchestration
    case timeRotation = "time_rotation"

    var id: String { rawValue }
    var title: String {
        switch self {
        case .orchestration: return "多模型编排"
        case .timeRotation: return "按时间轮转"
        }
    }
}

struct FusionTimeRoute: Identifiable, Equatable {
    let id = UUID()
    var startMinute: Int
    var endMinute: Int
    var model: String

    static func == (left: FusionTimeRoute, right: FusionTimeRoute) -> Bool {
        left.startMinute == right.startMinute
            && left.endMinute == right.endMinute
            && left.model == right.model
    }
}

func fusionTimeRouteValidationError(_ routes: [FusionTimeRoute]) -> String? {
    guard (1...24).contains(routes.count) else {
        return "请配置 1–24 个时段。"
    }
    var occupied = Array(repeating: false, count: 1_440)
    for route in routes {
        guard (0..<1_440).contains(route.startMinute),
              (0..<1_440).contains(route.endMinute),
              route.startMinute != route.endMinute
        else {
            return "时段的开始和结束时间不能相同。"
        }
        guard !route.model.isEmpty else { return "每个时段都必须选择模型。" }
        for minute in 0..<1_440 where fusionTimeRoute(route, contains: minute) {
            if occupied[minute] { return "时段不能重叠。" }
            occupied[minute] = true
        }
    }
    return nil
}

private func fusionTimeRoute(_ route: FusionTimeRoute, contains minute: Int) -> Bool {
    if route.startMinute < route.endMinute {
        return (route.startMinute..<route.endMinute).contains(minute)
    }
    return minute >= route.startMinute || minute < route.endMinute
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
    var mode = FusionSettingsMode.orchestration
    var timeRoutes: [FusionTimeRoute] = []
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
        value.mode = (profile["mode"] as? String)
            .flatMap(FusionSettingsMode.init(rawValue:)) ?? value.mode
        value.timeRoutes = (profile["time_routes"] as? [[String: Any]] ?? []).compactMap { route in
            guard let startMinute = (route["start_minute"] as? NSNumber)?.intValue,
                  let endMinute = (route["end_minute"] as? NSNumber)?.intValue,
                  let model = route["model"] as? String
            else {
                return nil
            }
            return FusionTimeRoute(
                startMinute: startMinute,
                endMinute: endMinute,
                model: model
            )
        }
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
            "mode": mode.rawValue,
            "time_routes": timeRoutes.map { route in
                [
                    "start_minute": route.startMinute,
                    "end_minute": route.endMinute,
                    "model": route.model,
                ] as [String: Any]
            },
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
