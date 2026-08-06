import Foundation

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
