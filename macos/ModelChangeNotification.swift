import Foundation
import UserNotifications

struct ModelChangeNotificationPayload: Equatable {
    static let command = "--deliver-model-notification"

    let title: String
    let subtitle: String
    let body: String

    init?(arguments: [String]) {
        let payload = Array(arguments.dropFirst())
        guard
            payload.count == 4,
            payload[0] == Self.command,
            !payload[1].isEmpty,
            !payload[2].isEmpty,
            !payload[3].isEmpty
        else {
            return nil
        }
        title = payload[1]
        subtitle = payload[2]
        body = payload[3]
    }
}

private final class NotificationDeliveryState: @unchecked Sendable {
    let semaphore = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private var completed = false
    private var failure: String?

    func complete(failure: String? = nil) {
        lock.lock()
        guard !completed else {
            lock.unlock()
            return
        }
        completed = true
        self.failure = failure
        lock.unlock()
        semaphore.signal()
    }

    func failureDescription() -> String? {
        lock.lock()
        defer { lock.unlock() }
        return failure
    }
}

enum ModelChangeNotification {
    private static let deliveryTimeout: TimeInterval = 10

    static func runIfRequested(arguments: [String]) -> Int32? {
        guard arguments.dropFirst().first == ModelChangeNotificationPayload.command else {
            return nil
        }
        guard let payload = ModelChangeNotificationPayload(arguments: arguments) else {
            writeError("invalid model notification arguments")
            return EXIT_FAILURE
        }
        return deliver(payload) ? EXIT_SUCCESS : EXIT_FAILURE
    }

    private static func deliver(_ payload: ModelChangeNotificationPayload) -> Bool {
        let center = UNUserNotificationCenter.current()
        let content = UNMutableNotificationContent()
        content.title = payload.title
        content.subtitle = payload.subtitle
        content.body = payload.body
        content.sound = .default
        content.threadIdentifier = "provider-model-changes"
        let request = UNNotificationRequest(
            identifier: "provider-model-change-\(UUID().uuidString)",
            content: content,
            trigger: nil
        )
        let state = NotificationDeliveryState()

        center.getNotificationSettings { settings in
            switch settings.authorizationStatus {
            case .authorized, .provisional:
                add(request, to: center, state: state)
            case .notDetermined:
                center.requestAuthorization(options: [.alert, .sound]) { granted, error in
                    if let error {
                        state.complete(failure: "request notification permission: \(error)")
                    } else if granted {
                        add(request, to: center, state: state)
                    } else {
                        state.complete(failure: "notification permission was not granted")
                    }
                }
            case .denied:
                state.complete(failure: "notifications are disabled for Codex Mixin")
            @unknown default:
                state.complete(failure: "unsupported notification authorization status")
            }
        }

        guard state.semaphore.wait(timeout: .now() + deliveryTimeout) == .success else {
            writeError("model notification delivery timed out")
            return false
        }
        if let failure = state.failureDescription() {
            writeError(failure)
            return false
        }
        return true
    }

    private static func add(
        _ request: UNNotificationRequest,
        to center: UNUserNotificationCenter,
        state: NotificationDeliveryState
    ) {
        center.add(request) { error in
            state.complete(failure: error.map { "deliver model notification: \($0)" })
        }
    }

    private static func writeError(_ message: String) {
        FileHandle.standardError.write(Data("Codex Mixin: \(message)\n".utf8))
    }
}
