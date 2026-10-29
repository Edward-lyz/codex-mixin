import Foundation

enum ProviderModelOrder: String, Decodable, CaseIterable, Hashable {
    case family
    case manual
}

// Keeps edit intent separate from incoming discovery snapshots and table sorting.
struct ProviderModelSelectionDraft {
    private(set) var modelIDs: [String]
    private(set) var order: ProviderModelOrder
    private(set) var savedModelIDs: [String]
    private(set) var savedOrder: ProviderModelOrder

    init(modelIDs: [String], order: ProviderModelOrder) {
        self.modelIDs = modelIDs
        self.order = order
        savedModelIDs = modelIDs
        savedOrder = order
    }

    var isDirty: Bool { modelIDs != savedModelIDs || order != savedOrder }

    mutating func setSelected(_ modelID: String, selected: Bool) {
        if selected {
            if !modelIDs.contains(modelID) { modelIDs.append(modelID) }
        } else {
            modelIDs.removeAll { $0 == modelID }
        }
    }

    mutating func move(_ modelID: String, to destination: Int) {
        guard let source = modelIDs.firstIndex(of: modelID),
              modelIDs.indices.contains(destination), source != destination
        else { return }
        modelIDs.remove(at: source)
        modelIDs.insert(modelID, at: destination)
        order = .manual
    }

    mutating func setOrder(_ value: ProviderModelOrder) {
        order = value
        if value == .family {
            let selected = Set(modelIDs)
            let saved = Set(savedModelIDs)
            modelIDs = savedModelIDs.filter(selected.contains)
                + modelIDs.filter { !saved.contains($0) }
        }
    }

    mutating func markSaved() {
        savedModelIDs = modelIDs
        savedOrder = order
    }

    mutating func discard() {
        modelIDs = savedModelIDs
        order = savedOrder
    }

    // Rebase membership edits on the latest snapshot. Only explicit local order
    // edits take precedence over a newer order from another shell.
    mutating func reload(modelIDs incoming: [String], order incomingOrder: ProviderModelOrder) -> Bool {
        let dirty = isDirty
        let conflict = dirty && (savedModelIDs != incoming || savedOrder != incomingOrder)
        if dirty {
            let selected = Set(modelIDs)
            let saved = Set(savedModelIDs)
            let removed = saved.subtracting(selected)
            let common = saved.intersection(selected)
            let orderEdited = order != savedOrder
                || (order == .manual
                    && modelIDs.filter(common.contains) != savedModelIDs.filter(common.contains))
            var merged = incoming.filter { !removed.contains($0) }
            for modelID in modelIDs where !saved.contains(modelID) && !merged.contains(modelID) {
                merged.append(modelID)
            }
            if orderEdited {
                let retained = Set(merged)
                let preferred = modelIDs.filter(retained.contains)
                let known = Set(preferred)
                modelIDs = preferred + merged.filter { !known.contains($0) }
            } else {
                modelIDs = merged
                order = incomingOrder
            }
        } else {
            modelIDs = incoming
            order = incomingOrder
        }
        savedModelIDs = incoming
        savedOrder = incomingOrder
        return conflict
    }

    func update(
        providerID: String,
        contexts: [String: UInt64],
        savedContexts: [String: UInt64]
    ) -> ProviderModelSelectionUpdate? {
        guard isDirty || contexts != savedContexts else { return nil }
        let changedModels = modelIDs != savedModelIDs
        return ProviderModelSelectionUpdate(
            providerID: providerID,
            modelIDs: changedModels ? modelIDs : nil,
            modelOrder: changedModels || order != savedOrder ? order : nil,
            modelContexts: contexts.filter { savedContexts[$0.key] != $0.value },
            clearedModelContexts: savedContexts.keys.filter { contexts[$0] == nil }.sorted()
        )
    }
}

struct ProviderModelSelectionUpdate {
    let providerID: String
    let modelIDs: [String]?
    let modelOrder: ProviderModelOrder?
    let modelContexts: [String: UInt64]
    let clearedModelContexts: [String]

    func arguments() throws -> [String] {
        if let modelIDs, modelIDs.isEmpty {
            throw GatewayError.command(AppLocalization.string("provider.modelOrder.emptySelection"))
        }
        guard modelIDs != nil || modelOrder != nil || !modelContexts.isEmpty
            || !clearedModelContexts.isEmpty else {
            throw GatewayError.command(AppLocalization.string("provider.modelOrder.noChanges"))
        }
        var result = ["providers", "select", providerID]
        if let modelIDs {
            for modelID in modelIDs { result += ["--model", modelID] }
        }
        if let modelOrder { result += ["--model-order", modelOrder.rawValue] }
        for (modelID, tokens) in modelContexts.sorted(by: { $0.key < $1.key }) {
            result += ["--model-context", "\(modelID)=\(tokens)"]
        }
        for modelID in clearedModelContexts.sorted() {
            result += ["--clear-model-context", modelID]
        }
        return result
    }
}
