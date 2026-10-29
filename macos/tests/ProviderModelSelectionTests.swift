import Foundation

enum GatewayError: Error {
    case command(String)
}

@main
struct ProviderModelSelectionTests {
    static func main() throws {
        let fixture = Data(#"""
        {
          "id":"test","display_name":"Test","enabled":true,
          "auxiliary_model_upstream":false,"protocol":"open_ai_responses",
          "base_url":"http://127.0.0.1:9","api_path":"/v1/responses",
          "model_source":{"kind":"static"},"api_key_configured":false,
          "quota_parser":"none","model_order":"manual",
          "selected_models":["gamma","alpha"],"new_models":[],
          "unavailable_selected_models":[],
          "cached_models":[{"id":"alpha"},{"id":"beta"},{"id":"gamma"}],
          "readiness":"healthy","readiness_issues":[],"routable_model_count":2
        }
        """#.utf8)
        let provider = try JSONDecoder().decode(ProviderView.self, from: fixture)
        require(provider.modelOrder == .manual, "decode the persisted ordering mode")
        var legacy = try JSONSerialization.jsonObject(with: fixture) as! [String: Any]
        legacy.removeValue(forKey: "model_order")
        let legacyProvider = try JSONDecoder().decode(ProviderView.self, from: JSONSerialization.data(withJSONObject: legacy))
        require(legacyProvider.modelOrder == .family, "older CLI snapshots default to family order")
        legacy["model_order"] = "unsupported"
        do {
            _ = try JSONDecoder().decode(ProviderView.self, from: JSONSerialization.data(withJSONObject: legacy))
            require(false, "unknown ordering modes must fail decoding")
        } catch is DecodingError { }

        var draft = ProviderModelSelectionDraft(modelIDs: provider.selectedModels, order: provider.modelOrder)
        require(draft.update(providerID: "test", contexts: [:], savedContexts: [:]) == nil,
                "unchanged drafts must not emit an empty select command")
        let context = draft.update(providerID: "test", contexts: ["alpha": 128_000], savedContexts: [:])!
        require(try context.arguments() == ["providers", "select", "test", "--model-context", "alpha=128000"],
                "context-only changes must omit selection and ordering flags")
        let clear = draft.update(providerID: "test", contexts: [:], savedContexts: ["alpha": 128_000])!
        require(try clear.arguments() == ["providers", "select", "test", "--clear-model-context", "alpha"],
                "context reset must preserve the model selection and mode")
        draft.setSelected("beta", selected: true)
        let selection = draft.update(providerID: "test", contexts: [:], savedContexts: [:])!
        require(try selection.arguments() == ["providers", "select", "test", "--model", "gamma",
                "--model", "alpha", "--model", "beta", "--model-order", "manual"],
                "append newly selected models and explicitly preserve manual mode")
        draft.discard()
        require(!draft.isDirty && draft.modelIDs == ["gamma", "alpha"], "discard restores order and membership")
        draft.setOrder(.family)
        require(try draft.update(providerID: "test", contexts: [:], savedContexts: [:])!.arguments()
                == ["providers", "select", "test", "--model-order", "family"],
                "restoring family ordering must use the existing CLI contract")
        draft.markSaved()
        draft.move("gamma", to: 0)
        draft.move("gamma", to: -1)
        require(!draft.isDirty && draft.order == .family, "no-op or invalid moves must not change the mode")
        draft.move("alpha", to: 0)
        require(draft.isDirty && draft.order == .manual && draft.modelIDs == ["alpha", "gamma"],
                "a pure reorder is a pending manual edit")
        let reordered = draft.update(providerID: "test", contexts: [:], savedContexts: [:])!
        require(reordered.modelIDs == ["alpha", "gamma"] && reordered.modelOrder == .manual,
                "serialize an order-only change without losing selection")
        require(!draft.reload(modelIDs: ["gamma", "alpha"], order: .family),
                "ordinary polling must not invent a conflict")
        require(draft.modelIDs == ["alpha", "gamma"] && draft.isDirty,
                "ordinary polling must preserve an unsaved reorder")
        require(draft.reload(modelIDs: ["gamma", "alpha", "delta"], order: .family),
                "detect external selection changes while an order draft is pending")
        require(draft.modelIDs == ["alpha", "gamma", "delta"],
                "preserve local order and append externally selected models")
        draft.discard()
        require(draft.modelIDs == ["gamma", "alpha", "delta"] && draft.order == .family,
                "discard uses the latest saved snapshot")

        var membership = ProviderModelSelectionDraft(modelIDs: ["gamma", "alpha"], order: .manual)
        membership.setSelected("beta", selected: true)
        require(membership.reload(modelIDs: ["alpha", "gamma", "delta"], order: .family),
                "report external edits while membership is pending")
        require(membership.modelIDs == ["alpha", "gamma", "delta", "beta"] && membership.order == .family,
                "membership-only edits must adopt the newer ordering mode and sequence")
        membership.setSelected("gamma", selected: false)
        _ = membership.reload(modelIDs: ["alpha", "gamma", "delta"], order: .family)
        require(!membership.modelIDs.contains("gamma"), "polling must not resurrect a locally removed model")

        var family = ProviderModelSelectionDraft(modelIDs: ["alpha", "gamma"], order: .family)
        family.setSelected("beta", selected: true)
        require(family.update(providerID: "test", contexts: [:], savedContexts: [:])!.modelOrder == .family,
                "saving membership changes must not implicitly switch family mode to manual")
        _ = family.reload(modelIDs: ["gamma", "alpha"], order: .manual)
        require(family.modelIDs == ["gamma", "alpha", "beta"] && family.order == .manual,
                "an external manual order must survive local membership edits")
        family.markSaved()
        require(!family.isDirty, "acknowledge a successful save")
        for modelID in family.modelIDs { family.setSelected(modelID, selected: false) }
        do {
            _ = try family.update(providerID: "test", contexts: [:], savedContexts: [:])!.arguments()
            require(false, "empty selections must not become a mode-only command that retains old models")
        } catch GatewayError.command { }
        print("Provider model selection logic: passed")
    }

    static func require(_ condition: Bool, _ message: String) {
        guard condition else {
            FileHandle.standardError.write(Data("FAILED: \(message)\n".utf8))
            exit(1)
        }
    }
}
