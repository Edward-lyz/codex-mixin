import Foundation

@main
struct FusionSettingsLogicTests {
    static func main() {
        let fresh = resolveFusionModelSelection(
            availableModelIDs: ["model-a", "model-b", "model-c", "model-d"],
            storedPanelModels: [],
            storedJudgeModel: "",
            storedFinalModel: ""
        )
        precondition(fresh.panelModels == ["model-a", "model-b", "model-c"])
        precondition(fresh.judgeModel == "model-a")
        precondition(fresh.finalModel == "model-b")

        let existing = resolveFusionModelSelection(
            availableModelIDs: ["model-a", "model-b", "unavailable-model"],
            storedPanelModels: ["unavailable-model"],
            storedJudgeModel: "unavailable-model",
            storedFinalModel: "model-b"
        )
        precondition(existing.panelModels == ["unavailable-model"])
        precondition(existing.judgeModel == "unavailable-model")
        precondition(existing.finalModel == "model-b")

        print("Fusion settings defaults: passed")
    }
}
