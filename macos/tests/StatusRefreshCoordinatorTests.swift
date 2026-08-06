import Foundation

@MainActor
final class ControlledRefresh {
    private(set) var runCount = 0
    private(set) var appliedCount = 0
    private var releases: [CheckedContinuation<Void, Never>] = []

    func run(isCurrent: @escaping StatusRefreshCoordinator.IsCurrent) async {
        runCount += 1
        await withCheckedContinuation { continuation in
            releases.append(continuation)
        }
        if isCurrent() {
            appliedCount += 1
        }
    }

    func waitForRunCount(_ expected: Int) async {
        while runCount < expected {
            await Task.yield()
        }
    }

    func releaseNext() {
        precondition(!releases.isEmpty, "No refresh is waiting to be released")
        releases.removeFirst().resume()
    }
}

@main
struct StatusRefreshCoordinatorTests {
    @MainActor
    static func main() async {
        let controlled = ControlledRefresh()
        let coordinator = StatusRefreshCoordinator { isCurrent in
            await controlled.run(isCurrent: isCurrent)
        }

        let first = Task { @MainActor in await coordinator.refresh() }
        await controlled.waitForRunCount(1)

        let second = Task { @MainActor in await coordinator.refresh() }
        let third = Task { @MainActor in await coordinator.refresh() }
        await Task.yield()
        precondition(controlled.runCount == 1, "Concurrent requests must share one in-flight refresh")

        controlled.releaseNext()
        await controlled.waitForRunCount(2)
        precondition(controlled.runCount == 2, "A burst during refresh must schedule one trailing refresh")
        precondition(controlled.appliedCount == 0, "Superseded refresh results must not be applied")

        controlled.releaseNext()
        await first.value
        await second.value
        await third.value
        precondition(controlled.runCount == 2, "The burst must not create more than one trailing refresh")
        precondition(controlled.appliedCount == 1, "Only the latest refresh result should be applied")

        let fourth = Task { @MainActor in await coordinator.refresh() }
        await controlled.waitForRunCount(3)
        controlled.releaseNext()
        await fourth.value
        precondition(controlled.runCount == 3, "A later request must start a fresh refresh")
        precondition(controlled.appliedCount == 2)
    }
}
