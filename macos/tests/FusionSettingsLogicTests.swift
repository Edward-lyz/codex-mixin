import Foundation

@main
struct FusionSettingsLogicTests {
    static func main() throws {
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

        let legacy = try FusionSettingsProfile.fromCLIJSON(
            """
            {
              "profile": {
                "id": "legacy",
                "panel_models": ["model-a", "model-b"],
                "judge_model": "model-a",
                "final_model": "model-b",
                "min_successful": 2,
                "max_completion_tokens": 4096,
                "timeout_ms": 120000,
                "show_intermediate_results": false,
                "panel_tools": {
                  "enabled": true,
                  "max_rounds": 4,
                  "max_calls_per_model": 8
                }
              }
            }
            """
        )
        precondition(legacy.id == "legacy")
        precondition(legacy.panelModels == ["model-a", "model-b"])
        precondition(legacy.minSuccessful == 2)
        precondition(legacy.maxCompletionTokens == 4096)
        precondition(legacy.timeoutMs == 120000)
        precondition(!legacy.showIntermediateResults)
        precondition(legacy.panelMaxRounds == 16)
        precondition(legacy.panelMaxCallsPerModel == 64)

        let current = try FusionSettingsProfile.fromCLIJSON(
            """
            {
              "profile": {
                "id": "current",
                "panel_models": ["model-c"],
                "judge_model": "model-c",
                "final_model": "model-c",
                "min_successful": 1,
                "max_completion_tokens": 2048,
                "timeout_ms": 300000,
                "show_intermediate_results": true,
                "panel_tools": {
                  "enabled": false,
                  "max_rounds": 12,
                  "max_calls_per_model": 33
                }
              }
            }
            """
        )
        precondition(current.id == "current")
        precondition(current.panelModels == ["model-c"])
        precondition(current.panelToolsEnabled == false)
        precondition(current.panelMaxRounds == 12)
        precondition(current.panelMaxCallsPerModel == 33)

        let roundTripSource = FusionSettingsProfile(
            id: "round-trip",
            panelModels: ["model-a", "model-b"],
            judgeModel: "model-a",
            finalModel: "model-b",
            minSuccessful: 2,
            maxCompletionTokens: 8192,
            timeoutMs: 180000,
            showIntermediateResults: false,
            panelToolsEnabled: true,
            panelMaxRounds: 20,
            panelMaxCallsPerModel: 40
        )
        let roundTripJSON = try roundTripSource.jsonString()
        let roundTrip = try FusionSettingsProfile.fromCLIJSON(
            "{\"profile\":\(roundTripJSON)}"
        )
        precondition(roundTrip.id == roundTripSource.id)
        precondition(roundTrip.panelModels == roundTripSource.panelModels)
        precondition(roundTrip.judgeModel == roundTripSource.judgeModel)
        precondition(roundTrip.finalModel == roundTripSource.finalModel)
        precondition(roundTrip.minSuccessful == roundTripSource.minSuccessful)
        precondition(roundTrip.maxCompletionTokens == roundTripSource.maxCompletionTokens)
        precondition(roundTrip.timeoutMs == roundTripSource.timeoutMs)
        precondition(roundTrip.showIntermediateResults == roundTripSource.showIntermediateResults)
        precondition(roundTrip.panelToolsEnabled == roundTripSource.panelToolsEnabled)
        precondition(roundTrip.panelMaxRounds == roundTripSource.panelMaxRounds)
        precondition(roundTrip.panelMaxCallsPerModel == roundTripSource.panelMaxCallsPerModel)

        print("Fusion settings logic: passed")
    }
}
