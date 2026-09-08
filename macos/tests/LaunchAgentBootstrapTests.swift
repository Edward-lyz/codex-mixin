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
        let menuPlist = menuLaunchAgentPlist(
            label: "local.codex-mixin.menu-launch",
            executablePath: "/Applications/Codex Mixin.app/Contents/MacOS/CodexMixinMenu",
            logPath: "/Users/test/.codex-mixin/macos-app.log"
        )
        precondition(menuPlist.contains("CodexMixinMenu"))
        precondition(!menuPlist.contains("<string>/usr/bin/open</string>"))
        precondition(menuPlist.contains("<key>KeepAlive</key>"))
        precondition(menuPlist.contains("<key>SuccessfulExit</key>"))
        precondition(menuPlist.contains("macos-app.log"))

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
