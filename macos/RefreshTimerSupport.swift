import Foundation

@MainActor
final class RefreshTimerController {
    typealias Handler = @MainActor () -> Void

    private let interval: TimeInterval
    private let handler: Handler
    private(set) var timer: Timer?

    init(interval: TimeInterval = 10, handler: @escaping Handler) {
        self.interval = interval
        self.handler = handler
    }

    var isRunning: Bool {
        timer?.isValid == true
    }

    func start() {
        stop()
        let timer = Timer(timeInterval: interval, repeats: true) { [weak self] _ in
            guard let self else { return }
            Task { @MainActor in
                self.handler()
            }
        }
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    func restart() {
        start()
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    func fireForTesting() {
        handler()
    }

    deinit {
        timer?.invalidate()
    }
}
