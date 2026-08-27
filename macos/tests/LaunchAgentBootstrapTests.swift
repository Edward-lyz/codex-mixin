import Foundation

private enum TestBootstrapError: Error, CustomStringConvertible {
    case transient
    case permanent

    var description: String {
        switch self {
        case .transient:
            return "Bootstrap failed: 5: Input/output error"
        case .permanent:
            return "Bootstrap failed: 122: Path had bad ownership/permissions"
        }
    }
}

@main
struct LaunchAgentBootstrapTests {
    static func main() async throws {
        var attempts = 0
        var delays = 0
        let output = try await retryLaunchAgentBootstrap(
            maxAttempts: 3,
            operation: {
                attempts += 1
                if attempts == 1 {
                    throw TestBootstrapError.transient
                }
                return "started"
            },
            delay: { delays += 1 }
        )
        precondition(output == "started")
        precondition(attempts == 2)
        precondition(delays == 1)

        attempts = 0
        do {
            _ = try await retryLaunchAgentBootstrap(
                maxAttempts: 3,
                operation: {
                    attempts += 1
                    throw TestBootstrapError.permanent
                },
                delay: {}
            )
            preconditionFailure("permanent launchctl errors must not be retried")
        } catch TestBootstrapError.permanent {
            precondition(attempts == 1)
        }

        print("LaunchAgent bootstrap retry: passed")
    }
}
