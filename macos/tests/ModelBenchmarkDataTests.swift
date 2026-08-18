import Foundation

@main
struct ModelBenchmarkDataTests {
    static func main() throws {
        let envelope = try JSONDecoder().decode(
            ModelBenchmarkSnapshotEnvelope.self,
            from: Data(
                """
                {
                  "snapshot": {
                    "run_id": "run-1",
                    "status": "completed",
                    "started_at": 1700000000000,
                    "updated_at": 1700000001000,
                    "finished_at": 1700000002000,
                    "timeout_seconds": 30,
                    "target_output_tokens": 100,
                    "total_models": 1,
                    "current_model": null,
                    "results": [
                      {
                        "model": "display-model",
                        "provider_id": "provider-1",
                        "provider_name": "Provider 1",
                        "upstream_model": "upstream-model",
                        "status": "completed",
                        "ttft_ms": 42,
                        "generation_ms": 100,
                        "total_ms": 142,
                        "output_tokens": 100,
                        "tps": 12.5,
                        "error": null
                      }
                    ],
                    "error": null,
                    "estimated_cost": 0.25,
                    "cost_currency": "USD",
                    "cost_error": null,
                    "provider_costs": [
                      {
                        "provider_id": "provider-1",
                        "currency": "USD",
                        "estimated_cost": 0.25,
                        "error": null
                      }
                    ]
                  }
                }
                """.utf8
            )
        )
        guard let snapshot = envelope.snapshot else {
            preconditionFailure("benchmark response did not contain a snapshot")
        }
        precondition(snapshot.runId == "run-1")
        precondition(snapshot.status == "completed")
        precondition(snapshot.targetOutputTokens == 100)
        precondition(snapshot.results.count == 1)
        let result = snapshot.results[0]
        precondition(result.providerID == "provider-1")
        precondition(result.upstreamModel == "upstream-model")
        precondition(result.ttftMs == 42)
        precondition(result.generationMs == 100)
        precondition(result.tps == 12.5)
        precondition(snapshot.providerCosts.first?.estimatedCost == 0.25)

        let previous = ModelBenchmarkResult(
            model: "old-model",
            providerID: "old-provider",
            providerName: "Old Provider",
            upstreamModel: "old-upstream",
            status: "completed",
            ttftMs: 90,
            generationMs: 900,
            totalMs: 990,
            outputTokens: 90,
            tps: 9.9,
            error: nil
        )
        let current = ModelBenchmarkResult(
            model: "new-model",
            providerID: "new-provider",
            providerName: "New Provider",
            upstreamModel: "new-upstream",
            status: "failed",
            ttftMs: 12,
            generationMs: 120,
            totalMs: 132,
            outputTokens: 12,
            tps: 1.2,
            error: "upstream failed"
        )
        let ttftOnly = mergedBenchmarkResult(
            current,
            previous: previous,
            targetOutputTokens: 1
        )
        precondition(ttftOnly.model == current.model)
        precondition(ttftOnly.providerID == current.providerID)
        precondition(ttftOnly.providerName == current.providerName)
        precondition(ttftOnly.upstreamModel == current.upstreamModel)
        precondition(ttftOnly.status == current.status)
        precondition(ttftOnly.ttftMs == current.ttftMs)
        precondition(ttftOnly.generationMs == previous.generationMs)
        precondition(ttftOnly.totalMs == current.totalMs)
        precondition(ttftOnly.outputTokens == current.outputTokens)
        precondition(ttftOnly.tps == previous.tps)
        precondition(ttftOnly.error == current.error)

        let complete = mergedBenchmarkResult(
            current,
            previous: previous,
            targetOutputTokens: 100
        )
        precondition(complete.model == current.model)
        precondition(complete.providerID == current.providerID)
        precondition(complete.providerName == current.providerName)
        precondition(complete.upstreamModel == current.upstreamModel)
        precondition(complete.status == current.status)
        precondition(complete.ttftMs == current.ttftMs)
        precondition(complete.generationMs == current.generationMs)
        precondition(complete.totalMs == current.totalMs)
        precondition(complete.outputTokens == current.outputTokens)
        precondition(complete.tps == current.tps)
        precondition(complete.error == current.error)
        print("Model benchmark DTO decode: passed")
    }
}
