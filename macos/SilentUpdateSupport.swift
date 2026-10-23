import Foundation

struct SilentUpdatePolicy: Codable {
    static let checkInterval: TimeInterval = 60 * 60
    static let idleWindow: TimeInterval = 30 * 60
    static let maximumAttempts = 3
    private(set) var version: String?
    private(set) var attempts = 0
    private(set) var retryAfter = Date.distantPast

    mutating func select(_ version: String) {
        guard self.version != version else { return }
        self.version = version
        attempts = 0
        retryAfter = .distantPast
    }

    mutating func beginAttempt(now: Date) -> Bool {
        guard version != nil, attempts < Self.maximumAttempts, now >= retryAfter else { return false }
        attempts += 1
        // Persist before handing off to Sparkle, including a crash/relaunch.
        retryAfter = now.addingTimeInterval(Self.checkInterval * pow(2, Double(attempts - 1)))
        return true
    }

    func canInstall(status: [String: Any]) -> Bool {
        guard status["gateway"] as? String == "running",
              let activity = status["activity"] as? [String: Any],
              let active = activity["active_requests"] as? Int,
              let idle = activity["idle_seconds"] as? Double
        else { return false }
        return active == 0 && idle >= Self.idleWindow
    }
}
