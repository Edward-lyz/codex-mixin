import Foundation

@main
struct SilentUpdateTests {
    static func main() throws {
        let start = Date(timeIntervalSince1970: 10_000)
        var policy = SilentUpdatePolicy()
        assert(!policy.beginAttempt(now: start))
        policy.select("1.0.0")
        assert(policy.beginAttempt(now: start))
        assert(!policy.beginAttempt(now: start.addingTimeInterval(3599)))
        assert(policy.beginAttempt(now: start.addingTimeInterval(3600)))
        assert(!policy.beginAttempt(now: start.addingTimeInterval(10_799)))
        assert(policy.beginAttempt(now: start.addingTimeInterval(10_800)))
        assert(!policy.beginAttempt(now: start.addingTimeInterval(100_000)))
        policy = try JSONDecoder().decode(SilentUpdatePolicy.self, from: JSONEncoder().encode(policy))
        policy.select("1.0.0")
        assert(!policy.beginAttempt(now: start.addingTimeInterval(100_000)))
        policy.select("1.0.1")
        assert(policy.beginAttempt(now: start))
        for (active, idle, allowed) in [(0, 1799, false), (0, 1800, true), (1, 4000, false)] {
            assert(policy.canInstall(status: ["gateway":"running", "activity":["active_requests":active,"idle_seconds":Double(idle)]]) == allowed)
        }
        assert(!policy.canInstall(status: ["gateway":"running"]))
        assert(!policy.canInstall(status: ["gateway":"stopped", "activity":["active_requests":0,"idle_seconds":4000.0]]))
        print("SilentUpdateTests passed")
    }
}
