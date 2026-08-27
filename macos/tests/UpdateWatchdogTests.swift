import Foundation
import CryptoKit

func appendAppDiagnosticLog(_ message: String, directory: URL) {}

@main
struct UpdateWatchdogTests {
    static func main() {
        let startedAt: UInt64 = 1_000
        let timeoutNanoseconds: UInt64 = 60_000_000_000
        let deadline = UpdateInstallDeadline(startedAt: startedAt, timeout: 60)

        precondition(!deadline.isExpired(at: startedAt + timeoutNanoseconds - 1))
        precondition(deadline.isExpired(at: startedAt + timeoutNanoseconds))
        precondition(
            deadline.remaining(at: startedAt + 10_000_000_000) == 50,
            "watchdog deadline must expose deterministic remaining time"
        )
        let launchRequest = UpdateWatchdogLaunchRequest(
            downloadURL: URL(string: "https://example.com/codex-mixin-0.5.3.dmg")!,
            contentLength: 123,
            edSignature: "signed-appcast-value",
            expectedVersion: "0.5.3",
            destinationURL: URL(fileURLWithPath: "/Applications/Codex Mixin.app"),
            parentPID: 42,
            deadlineUptimeNanoseconds: deadline.deadlineUptimeNanoseconds
        )
        precondition(
            launchRequest.arguments == [
                "--update-watchdog",
                "--parent-pid", "42",
                "--download-url", "https://example.com/codex-mixin-0.5.3.dmg",
                "--destination", "/Applications/Codex Mixin.app",
                "--expected-version", "0.5.3",
                "--content-length", "123",
                "--ed-signature", "signed-appcast-value",
                "--deadline-uptime-ns", String(startedAt + timeoutNanoseconds),
            ],
            "Sparkle must pass the complete signed appcast metadata to the watchdog"
        )
        var launchGate = UpdateWatchdogLaunchGate()
        precondition(launchGate.reserve(launchRequest))
        precondition(
            !launchGate.reserve(launchRequest),
            "the same update must start only one watchdog"
        )
        launchGate.release(launchRequest)
        precondition(
            launchGate.reserve(launchRequest),
            "a failed watchdog launch must be retryable"
        )

        var timedOut = UpdateWatchdogState(
            expectedVersion: "0.5.3",
            deadlineUptimeNanoseconds: startedAt + timeoutNanoseconds
        )
        precondition(
            timedOut.observe(
                parentIsRunning: true,
                installedVersion: "0.5.2",
                nowUptimeNanoseconds: startedAt
            ) == .waiting
        )
        var fallbackCount = 0
        for now in [
            startedAt + timeoutNanoseconds,
            startedAt + timeoutNanoseconds + 1,
            startedAt + timeoutNanoseconds + 2,
        ] {
            if timedOut.observe(
                parentIsRunning: true,
                installedVersion: "0.5.2",
                nowUptimeNanoseconds: now
            ) == .fallback {
                fallbackCount += 1
            }
        }
        precondition(fallbackCount == 1, "a timed-out update must trigger fallback once")
        precondition(
            timedOut.observe(
                parentIsRunning: true,
                installedVersion: "0.5.2",
                nowUptimeNanoseconds: startedAt + timeoutNanoseconds + 3
            ) == .alreadyHandled
        )

        var completed = UpdateWatchdogState(
            expectedVersion: "0.5.3",
            deadlineUptimeNanoseconds: startedAt + timeoutNanoseconds
        )
        precondition(
            completed.observe(
                parentIsRunning: false,
                installedVersion: "0.5.3",
                nowUptimeNanoseconds: startedAt + timeoutNanoseconds
            ) == .completed,
            "an install that is ready at the deadline must not fall back"
        )
        precondition(
            completed.observe(
                parentIsRunning: true,
                installedVersion: "0.5.2",
                nowUptimeNanoseconds: startedAt + timeoutNanoseconds + 1
            ) == .completed,
            "completed updates must never fall back later"
        )
        precondition(isInstalledVersionAtLeast("0.5.4", expectedVersion: "0.5.3"))
        precondition(isInstalledVersionAtLeast("0.5.10", expectedVersion: "0.5.9"))
        precondition(!isInstalledVersionAtLeast("0.5.2", expectedVersion: "0.5.3"))

        var newerInstalled = UpdateWatchdogState(
            expectedVersion: "0.5.3",
            deadlineUptimeNanoseconds: startedAt + timeoutNanoseconds
        )
        precondition(
            newerInstalled.observe(
                parentIsRunning: false,
                installedVersion: "0.5.4",
                nowUptimeNanoseconds: startedAt + timeoutNanoseconds
            ) == .completed,
            "a newer installed version satisfies the update"
        )

        let temporaryDMG = FileManager.default.temporaryDirectory
            .appendingPathComponent("update-watchdog-\(UUID().uuidString).dmg")
        try! Data(repeating: 0, count: 4).write(to: temporaryDMG)
        defer { try? FileManager.default.removeItem(at: temporaryDMG) }
        precondition(
            !UpdateWatchdog.fallbackDMGHasExpectedSize(at: temporaryDMG, contentLength: 5),
            "a same-name DMG with the wrong size must not be trusted"
        )
        precondition(
            UpdateWatchdog.fallbackDMGHasExpectedSize(at: temporaryDMG, contentLength: 4),
            "a fallback DMG is valid only when its size matches the appcast"
        )
        let signingKey = Curve25519.Signing.PrivateKey()
        let archiveData = try! Data(contentsOf: temporaryDMG)
        let signature = try! signingKey.signature(for: archiveData).base64EncodedString()
        let publicKey = signingKey.publicKey.rawRepresentation.base64EncodedString()
        precondition(
            UpdateWatchdog.fallbackDMGHasValidSignature(
                at: temporaryDMG,
                edSignature: signature,
                publicEDKey: publicKey
            ),
            "a fallback DMG must pass Ed25519 verification"
        )
        precondition(
            !UpdateWatchdog.fallbackDMGHasValidSignature(
                at: temporaryDMG,
                edSignature: Data(repeating: 0, count: 64).base64EncodedString(),
                publicEDKey: publicKey
            ),
            "a fallback DMG with a forged signature must be rejected"
        )

        precondition(
            UpdateWatchdog.fallbackDMGName(from: URL(string: "https://example.com/codex-mixin-0.5.3-aarch64-apple-darwin.dmg")!)
                == "codex-mixin-0.5.3-aarch64-apple-darwin.dmg"
        )
        precondition(
            UpdateWatchdog.fallbackDMGName(from: URL(string: "https://example.com/path/../unsafe.zip")!)
                == "codex-mixin-update.dmg",
            "fallback must not use a non-DMG or traversal-like filename"
        )

        print("Update watchdog state and deadline tests passed")
    }
}
