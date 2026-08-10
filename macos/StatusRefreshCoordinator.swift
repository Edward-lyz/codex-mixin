import Foundation

enum StatusRefreshScope: Int {
    case health
    case status
    case full

    func merged(with other: Self) -> Self {
        rawValue >= other.rawValue ? self : other
    }
}

struct QuotaRefreshPolicy {
    let ttl: TimeInterval
    private(set) var lastAttemptAt: Date?

    init(ttl: TimeInterval = 10, lastAttemptAt: Date? = nil) {
        self.ttl = ttl
        self.lastAttemptAt = lastAttemptAt
    }

    func isDue(at now: Date = Date()) -> Bool {
        guard let lastAttemptAt else { return true }
        return now.timeIntervalSince(lastAttemptAt) >= ttl
    }

    mutating func markAttempt(at now: Date = Date()) {
        lastAttemptAt = now
    }
}

final class StatusRefreshCoordinator {
    typealias IsCurrent = @MainActor () -> Bool
    typealias Operation = @MainActor (_ isCurrent: @escaping IsCurrent) async -> Void

    private let operation: Operation
    private var isRefreshing = false
    private var refreshRequested = false
    private var generation = 0
    private var waiters: [CheckedContinuation<Void, Never>] = []

    init(operation: @escaping Operation) {
        self.operation = operation
    }

    @MainActor
    func refresh() async {
        await withCheckedContinuation { continuation in
            waiters.append(continuation)
            refreshRequested = true
            generation += 1

            guard !isRefreshing else { return }
            isRefreshing = true
            Task { @MainActor in
                await drainRefreshes()
            }
        }
    }

    @MainActor
    private func drainRefreshes() async {
        while refreshRequested {
            refreshRequested = false
            let operationGeneration = generation
            await operation { [weak self] in
                self?.generation == operationGeneration
            }
        }

        isRefreshing = false
        let completedWaiters = waiters
        waiters.removeAll(keepingCapacity: true)
        completedWaiters.forEach { $0.resume() }
    }
}
