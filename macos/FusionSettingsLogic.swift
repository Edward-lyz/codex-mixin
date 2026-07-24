import Foundation

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
