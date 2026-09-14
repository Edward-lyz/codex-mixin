import Foundation

private let launchAgentBootstrapRetryDelayNanoseconds: UInt64 = 500_000_000
private let launchAgentBootstrapAttemptLimit = 10
private let gatewayReadinessPollDelayNanoseconds: UInt64 = 1_000_000_000
let gatewayReadinessAttemptLimit = 90

struct GatewayReadinessTimeout: Error {
    let lastFailure: String
}

func menuLaunchAgentPlist(
    label: String,
    executablePath: String,
    logPath: String
) -> String {
    """
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
      <key>Label</key>
      <string>\(launchAgentXMLescape(label))</string>
      <key>ProgramArguments</key>
      <array>
        <string>\(launchAgentXMLescape(executablePath))</string>
      </array>
      <key>RunAtLoad</key>
      <true/>
      <key>KeepAlive</key>
      <dict>
        <key>SuccessfulExit</key>
        <false/>
      </dict>
      <key>ThrottleInterval</key>
      <integer>10</integer>
      <key>ProcessType</key>
      <string>Interactive</string>
      <key>StandardOutPath</key>
      <string>\(launchAgentXMLescape(logPath))</string>
      <key>StandardErrorPath</key>
      <string>\(launchAgentXMLescape(logPath))</string>
    </dict>
    </plist>
    """
}

private func launchAgentXMLescape(_ value: String) -> String {
    value
        .replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
        .replacingOccurrences(of: "\"", with: "&quot;")
        .replacingOccurrences(of: "'", with: "&apos;")
}

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

func retryGatewayReadiness(
    maxAttempts: Int = gatewayReadinessAttemptLimit,
    operation: () async throws -> String,
    delay: () async throws -> Void = {
        try await Task.sleep(nanoseconds: gatewayReadinessPollDelayNanoseconds)
    }
) async throws -> String {
    let attemptLimit = max(1, maxAttempts)
    var lastFailure = "gateway has not reported a healthy status"
    for attempt in 1...attemptLimit {
        do {
            let status = try await operation()
            if status.contains("gateway: running") {
                return status
            }
            lastFailure = status
        } catch {
            lastFailure = String(describing: error)
        }
        if attempt < attemptLimit {
            try await delay()
        }
    }
    throw GatewayReadinessTimeout(lastFailure: lastFailure)
}
