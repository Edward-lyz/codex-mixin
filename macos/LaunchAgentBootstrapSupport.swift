import Foundation

private let launchAgentBootstrapRetryDelayNanoseconds: UInt64 = 500_000_000
private let launchAgentBootstrapAttemptLimit = 10

func retryLaunchAgentBootstrap(
    maxAttempts: Int = launchAgentBootstrapAttemptLimit,
    operation: () async throws -> String,
    delay: () async throws -> Void = {
        try await Task.sleep(nanoseconds: launchAgentBootstrapRetryDelayNanoseconds)
    }
) async throws -> String {
    let attemptLimit = max(1, maxAttempts)
    var attempt = 1
    while true {
        do {
            return try await operation()
        } catch {
            let message = String(describing: error)
            let isTransient = message.contains("Bootstrap failed: 5: Input/output error")
                || message == "exit 5"
            guard isTransient, attempt < attemptLimit else {
                throw error
            }
            attempt += 1
            try await delay()
        }
    }
}
