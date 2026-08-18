import Foundation

struct ModelBenchmarkSnapshotEnvelope: Decodable {
    let snapshot: ModelBenchmarkSnapshot?
}

struct ModelBenchmarkSnapshot: Decodable {
    let runId: String
    let status: String
    let startedAt: UInt64
    let updatedAt: UInt64
    let finishedAt: UInt64?
    let timeoutSeconds: UInt64
    let targetOutputTokens: UInt64
    let totalModels: Int
    let currentModel: String?
    let results: [ModelBenchmarkResult]
    let error: String?
    let estimatedCost: Double?
    let costCurrency: String?
    let costError: String?
    let providerCosts: [ProviderBenchmarkCost]

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case status
        case startedAt = "started_at"
        case updatedAt = "updated_at"
        case finishedAt = "finished_at"
        case timeoutSeconds = "timeout_seconds"
        case targetOutputTokens = "target_output_tokens"
        case totalModels = "total_models"
        case currentModel = "current_model"
        case results
        case error
        case estimatedCost = "estimated_cost"
        case costCurrency = "cost_currency"
        case costError = "cost_error"
        case providerCosts = "provider_costs"
    }
}

struct ProviderBenchmarkCost: Decodable {
    let providerID: String
    let currency: String?
    let estimatedCost: Double?
    let error: String?

    enum CodingKeys: String, CodingKey {
        case providerID = "provider_id"
        case currency
        case estimatedCost = "estimated_cost"
        case error
    }
}

struct ModelBenchmarkResult: Decodable {
    let model: String
    let providerID: String
    let providerName: String
    let upstreamModel: String
    let status: String
    let ttftMs: UInt64?
    let generationMs: UInt64?
    let totalMs: UInt64
    let outputTokens: UInt64?
    let tps: Double?
    let error: String?

    enum CodingKeys: String, CodingKey {
        case model
        case providerID = "provider_id"
        case providerName = "provider_name"
        case upstreamModel = "upstream_model"
        case status
        case ttftMs = "ttft_ms"
        case generationMs = "generation_ms"
        case totalMs = "total_ms"
        case outputTokens = "output_tokens"
        case tps
        case error
    }
}

func mergedBenchmarkResult(
    _ result: ModelBenchmarkResult,
    previous: ModelBenchmarkResult?,
    targetOutputTokens: UInt64
) -> ModelBenchmarkResult {
    guard targetOutputTokens == 1 else { return result }
    return ModelBenchmarkResult(
        model: result.model,
        providerID: result.providerID,
        providerName: result.providerName,
        upstreamModel: result.upstreamModel,
        status: result.status,
        ttftMs: result.ttftMs,
        generationMs: previous?.generationMs,
        totalMs: result.totalMs,
        outputTokens: result.outputTokens,
        tps: previous?.tps,
        error: result.error
    )
}
