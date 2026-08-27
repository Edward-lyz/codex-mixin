import Cocoa
import CryptoKit
import Darwin

struct UpdateInstallDeadline: Sendable {
    static let defaultTimeout: TimeInterval = 60
    private static let nanosecondsPerSecond: Double = 1_000_000_000

    let deadlineUptimeNanoseconds: UInt64

    init(
        startedAt uptimeNanoseconds: UInt64 = DispatchTime.now().uptimeNanoseconds,
        timeout: TimeInterval = Self.defaultTimeout
    ) {
        let duration = UInt64(max(timeout, 0) * Self.nanosecondsPerSecond)
        deadlineUptimeNanoseconds = UInt64.max - uptimeNanoseconds < duration
            ? UInt64.max
            : uptimeNanoseconds + duration
    }

    init(deadlineUptimeNanoseconds: UInt64) {
        self.deadlineUptimeNanoseconds = deadlineUptimeNanoseconds
    }

    func isExpired(at uptimeNanoseconds: UInt64 = DispatchTime.now().uptimeNanoseconds) -> Bool {
        uptimeNanoseconds >= deadlineUptimeNanoseconds
    }

    func remaining(at uptimeNanoseconds: UInt64 = DispatchTime.now().uptimeNanoseconds) -> TimeInterval {
        guard uptimeNanoseconds < deadlineUptimeNanoseconds else { return 0 }
        return TimeInterval(deadlineUptimeNanoseconds - uptimeNanoseconds) / Self.nanosecondsPerSecond
    }
}

struct UpdateWatchdogLaunchRequest: Equatable {
    let downloadURL: URL
    let contentLength: UInt64
    let edSignature: String
    let expectedVersion: String
    let destinationURL: URL
    let parentPID: pid_t
    let deadlineUptimeNanoseconds: UInt64

    var identity: UpdateWatchdogLaunchIdentity {
        UpdateWatchdogLaunchIdentity(
            downloadURL: downloadURL,
            destinationURL: destinationURL,
            expectedVersion: expectedVersion
        )
    }

    var arguments: [String] {
        [
            UpdateWatchdog.command,
            UpdateWatchdog.parentPIDArgument,
            String(parentPID),
            UpdateWatchdog.downloadURLArgument,
            downloadURL.absoluteString,
            UpdateWatchdog.destinationArgument,
            destinationURL.path,
            UpdateWatchdog.expectedVersionArgument,
            expectedVersion,
            UpdateWatchdog.contentLengthArgument,
            String(contentLength),
            UpdateWatchdog.edSignatureArgument,
            edSignature,
            UpdateWatchdog.deadlineArgument,
            String(deadlineUptimeNanoseconds),
        ]
    }
}

struct UpdateWatchdogLaunchIdentity: Hashable {
    let downloadURL: URL
    let destinationURL: URL
    let expectedVersion: String
}

struct UpdateWatchdogLaunchGate {
    private var started: Set<UpdateWatchdogLaunchIdentity> = []

    mutating func reserve(_ request: UpdateWatchdogLaunchRequest) -> Bool {
        started.insert(request.identity).inserted
    }

    mutating func release(_ request: UpdateWatchdogLaunchRequest) {
        started.remove(request.identity)
    }
}

enum UpdateWatchdogDecision: Equatable {
    case waiting
    case completed
    case fallback
    case alreadyHandled
}

struct UpdateWatchdogState {
    let expectedVersion: String
    let deadlineUptimeNanoseconds: UInt64
    private var outcome: Outcome = .waiting

    init(expectedVersion: String, deadlineUptimeNanoseconds: UInt64) {
        self.expectedVersion = expectedVersion
        self.deadlineUptimeNanoseconds = deadlineUptimeNanoseconds
    }

    private enum Outcome {
        case waiting
        case completed
        case fallback
    }

    mutating func observe(
        parentIsRunning: Bool,
        installedVersion: String?,
        nowUptimeNanoseconds: UInt64
    ) -> UpdateWatchdogDecision {
        switch outcome {
        case .completed:
            return .completed
        case .fallback:
            return .alreadyHandled
        case .waiting:
            break
        }

        if !parentIsRunning,
           isInstalledVersionAtLeast(installedVersion, expectedVersion: expectedVersion)
        {
            outcome = .completed
            return .completed
        }
        guard nowUptimeNanoseconds < deadlineUptimeNanoseconds else {
            outcome = .fallback
            return .fallback
        }
        return .waiting
    }
}

func isInstalledVersionAtLeast(
    _ installedVersion: String?,
    expectedVersion: String
) -> Bool {
    guard let installedVersion,
          let installedParts = numericVersionParts(installedVersion),
          let expectedParts = numericVersionParts(expectedVersion)
    else {
        return false
    }

    let count = max(installedParts.count, expectedParts.count)
    for index in 0..<count {
        let installed = index < installedParts.count ? installedParts[index] : 0
        let expected = index < expectedParts.count ? expectedParts[index] : 0
        if installed != expected {
            return installed > expected
        }
    }
    return true
}

private func numericVersionParts(_ version: String) -> [Int]? {
    let parts = version.split(separator: ".", omittingEmptySubsequences: false)
    guard !parts.isEmpty else { return nil }
    let numbers = parts.compactMap { Int($0) }
    guard numbers.count == parts.count, numbers.allSatisfy({ $0 >= 0 }) else {
        return nil
    }
    return numbers
}

enum UpdateWatchdog {
    static let command = "--update-watchdog"
    static let parentPIDArgument = "--parent-pid"
    static let downloadURLArgument = "--download-url"
    static let destinationArgument = "--destination"
    static let expectedVersionArgument = "--expected-version"
    static let contentLengthArgument = "--content-length"
    static let edSignatureArgument = "--ed-signature"
    static let deadlineArgument = "--deadline-uptime-ns"
    private static let publicEDKeyInfoKey = "SUPublicEDKey"

    static func runIfRequested(arguments: [String]) -> Bool {
        guard arguments.contains(command) else { return false }
        run(arguments: arguments)
        return true
    }

    static func fallbackDMGName(from url: URL) -> String {
        guard let filename = url.lastPathComponent.removingPercentEncoding,
              filename.lowercased().hasSuffix(".dmg"),
              !filename.contains("/"),
              !filename.contains("\\"),
              filename != ".",
              filename != ".."
        else {
            return "codex-mixin-update.dmg"
        }
        return filename
    }

    private static func run(arguments: [String]) {
        let logDirectory = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".codex-mixin")
        do {
            let parentPID = try requiredPID(in: arguments)
            let downloadURL = try requiredHTTPSURL(
                argument: downloadURLArgument,
                in: arguments
            )
            let destinationURL = try requiredURL(
                argument: destinationArgument,
                in: arguments
            )
            let expectedVersion = try requiredArgument(
                expectedVersionArgument,
                in: arguments
            )
            let contentLength = try requiredContentLength(in: arguments)
            let edSignature = try requiredArgument(edSignatureArgument, in: arguments)
            let deadline = try requiredDeadline(in: arguments)

            appendAppDiagnosticLog(
                "APP_UPDATE_WATCHDOG started parent_pid=\(parentPID) expected_version=\(expectedVersion) content_length=\(contentLength) deadline_uptime_ns=\(deadline.deadlineUptimeNanoseconds)",
                directory: logDirectory
            )

            do {
                try waitForInstallation(
                    parentPID: parentPID,
                    destinationURL: destinationURL,
                    expectedVersion: expectedVersion,
                    deadline: deadline,
                    logDirectory: logDirectory
                )
                appendAppDiagnosticLog(
                    "APP_UPDATE_WATCHDOG completed expected_version=\(expectedVersion)",
                    directory: logDirectory
                )
                return
            } catch WatchdogError.timedOut {
                appendAppDiagnosticLog(
                    "APP_UPDATE_WATCHDOG timeout; switching to manual DMG install",
                    directory: logDirectory
                )
            }

            let dmgURL = try prepareFallbackDMG(
                from: downloadURL,
                contentLength: contentLength,
                edSignature: edSignature,
                logDirectory: logDirectory
            )
            guard NSWorkspace.shared.open(dmgURL) else {
                throw WatchdogError.fallbackOpenFailed(dmgURL.path)
            }
            appendAppDiagnosticLog(
                "APP_UPDATE_WATCHDOG manual fallback opened dmg=\(dmgURL.path) expected_version=\(expectedVersion)",
                directory: logDirectory
            )
            showFallbackNotice(expectedVersion: expectedVersion)
        } catch {
            appendAppDiagnosticLog(
                "APP_UPDATE_WATCHDOG failed \(String(describing: error))",
                directory: logDirectory
            )
            showFallbackFailure(error)
            exit(EXIT_FAILURE)
        }
    }

    private static func waitForInstallation(
        parentPID: pid_t,
        destinationURL: URL,
        expectedVersion: String,
        deadline: UpdateInstallDeadline,
        logDirectory: URL
    ) throws {
        var state = UpdateWatchdogState(
            expectedVersion: expectedVersion,
            deadlineUptimeNanoseconds: deadline.deadlineUptimeNanoseconds
        )
        while !deadline.isExpired() {
            let now = DispatchTime.now().uptimeNanoseconds
            let observedVersion = appVersion(at: destinationURL)
            switch state.observe(
                parentIsRunning: processIsRunning(parentPID),
                installedVersion: observedVersion,
                nowUptimeNanoseconds: now
            ) {
            case .completed:
                return
            case .fallback:
                appendAppDiagnosticLog(
                    "APP_UPDATE_WATCHDOG installation deadline reached parent_pid=\(parentPID) observed_version=\(observedVersion ?? "missing")",
                    directory: logDirectory
                )
                throw WatchdogError.timedOut
            case .waiting, .alreadyHandled:
                break
            }
            let sleepMicroseconds = UInt32(min(deadline.remaining(at: now) * 1_000_000, 100_000))
            if sleepMicroseconds > 0 {
                usleep(sleepMicroseconds)
            }
        }
        // The final observation handles the exact deadline boundary. This branch only
        // protects against a monotonic clock race or a zero-length sleep.
        let observedVersion = appVersion(at: destinationURL)
        if !processIsRunning(parentPID),
           isInstalledVersionAtLeast(observedVersion, expectedVersion: expectedVersion)
        {
            return
        }
        appendAppDiagnosticLog(
            "APP_UPDATE_WATCHDOG installation deadline reached parent_pid=\(parentPID) observed_version=\(observedVersion ?? "missing")",
            directory: logDirectory
        )
        throw WatchdogError.timedOut
    }

    static func fallbackDMGHasExpectedSize(at url: URL, contentLength: UInt64) -> Bool {
        guard contentLength > 0,
              let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
              let size = attributes[.size] as? NSNumber
        else {
            return false
        }
        return size.uint64Value == contentLength
    }

    private static func prepareFallbackDMG(
        from remoteURL: URL,
        contentLength: UInt64,
        edSignature: String,
        logDirectory: URL
    ) throws -> URL {
        guard contentLength > 0 else {
            throw WatchdogError.invalidContentLength(String(contentLength))
        }
        let downloads = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Downloads")
        try FileManager.default.createDirectory(
            at: downloads,
            withIntermediateDirectories: true
        )
        let destination = downloads.appendingPathComponent(fallbackDMGName(from: remoteURL))
        if FileManager.default.fileExists(atPath: destination.path) {
            let quarantineURL = destination.appendingPathExtension("stale-\(UUID().uuidString)")
            do {
                try FileManager.default.moveItem(at: destination, to: quarantineURL)
                appendAppDiagnosticLog(
                    "APP_UPDATE_WATCHDOG quarantined stale fallback dmg=\(destination.path) quarantine=\(quarantineURL.path)",
                    directory: logDirectory
                )
            } catch {
                throw WatchdogError.fallbackFileQuarantineFailed(
                    destination.path,
                    String(describing: error)
                )
            }
        }

        let temporary = destination.appendingPathExtension("download")
        try? FileManager.default.removeItem(at: temporary)
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/curl")
        process.arguments = [
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "15",
            "--max-time",
            "180",
            "--output",
            temporary.path,
            remoteURL.absoluteString,
        ]
        process.standardOutput = FileHandle.nullDevice
        let stderrPipe = Pipe()
        process.standardError = stderrPipe
        try process.run()
        let stderrData = stderrPipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        let stderr = String(data: stderrData, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let boundedStderr = String(stderr.prefix(4_000))
        guard process.terminationStatus == 0 else {
            try? FileManager.default.removeItem(at: temporary)
            appendAppDiagnosticLog(
                "APP_UPDATE_WATCHDOG fallback curl failed status=\(process.terminationStatus) stderr=\(boundedStderr.isEmpty ? "<empty>" : boundedStderr)",
                directory: logDirectory
            )
            throw WatchdogError.fallbackDownloadFailed(
                process.terminationStatus,
                stderr: boundedStderr
            )
        }
        guard fallbackDMGHasExpectedSize(at: temporary, contentLength: contentLength) else {
            let actualSize = fileSize(at: temporary)
            try? FileManager.default.removeItem(at: temporary)
            appendAppDiagnosticLog(
                "APP_UPDATE_WATCHDOG fallback DMG size mismatch expected=\(contentLength) actual=\(actualSize.map(String.init) ?? "missing")",
                directory: logDirectory
            )
            throw WatchdogError.fallbackDownloadSizeMismatch(
                expected: contentLength,
                actual: actualSize
            )
        }
        do {
            try verifyFallbackDMG(at: temporary, edSignature: edSignature)
        } catch {
            try? FileManager.default.removeItem(at: temporary)
            appendAppDiagnosticLog(
                "APP_UPDATE_WATCHDOG fallback DMG signature verification failed",
                directory: logDirectory
            )
            throw error
        }
        try FileManager.default.moveItem(at: temporary, to: destination)
        guard fallbackDMGHasExpectedSize(at: destination, contentLength: contentLength) else {
            throw WatchdogError.fallbackDownloadSizeMismatch(
                expected: contentLength,
                actual: fileSize(at: destination)
            )
        }
        try verifyFallbackDMG(at: destination, edSignature: edSignature)
        return destination
    }

    static func fallbackDMGHasValidSignature(
        at url: URL,
        edSignature: String,
        publicEDKey: String
    ) -> Bool {
        guard let signatureData = Data(base64Encoded: edSignature),
              signatureData.count == 64,
              let publicKeyData = Data(base64Encoded: publicEDKey),
              publicKeyData.count == 32,
              let archiveData = try? Data(contentsOf: url, options: [.mappedIfSafe]),
              let publicKey = try? Curve25519.Signing.PublicKey(
                  rawRepresentation: publicKeyData
              )
        else {
            return false
        }
        return publicKey.isValidSignature(signatureData, for: archiveData)
    }

    private static func verifyFallbackDMG(at url: URL, edSignature: String) throws {
        guard let publicEDKey = Bundle.main.object(
            forInfoDictionaryKey: publicEDKeyInfoKey
        ) as? String,
            fallbackDMGHasValidSignature(
                at: url,
                edSignature: edSignature,
                publicEDKey: publicEDKey
            )
        else {
            throw WatchdogError.fallbackSignatureInvalid
        }
    }

    private static func fileSize(at url: URL) -> UInt64? {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
              let size = attributes[.size] as? NSNumber
        else {
            return nil
        }
        return size.uint64Value
    }

    private static func appVersion(at applicationURL: URL) -> String? {
        let infoURL = applicationURL.appendingPathComponent("Contents/Info.plist")
        guard let data = try? Data(contentsOf: infoURL),
              let plist = try? PropertyListSerialization.propertyList(
                  from: data,
                  options: [],
                  format: nil
              ) as? [String: Any]
        else {
            return nil
        }
        return plist["CFBundleVersion"] as? String
    }

    private static func processIsRunning(_ pid: pid_t) -> Bool {
        kill(pid, 0) == 0 || errno == EPERM
    }

    private static func requiredArgument(_ argument: String, in arguments: [String]) throws -> String {
        guard let index = arguments.firstIndex(of: argument), index + 1 < arguments.count else {
            throw WatchdogError.missingArgument(argument)
        }
        let value = arguments[index + 1]
        guard !value.isEmpty else {
            throw WatchdogError.missingArgument(argument)
        }
        return value
    }

    private static func requiredURL(argument: String, in arguments: [String]) throws -> URL {
        URL(fileURLWithPath: try requiredArgument(argument, in: arguments))
    }

    private static func requiredHTTPSURL(argument: String, in arguments: [String]) throws -> URL {
        let value = try requiredArgument(argument, in: arguments)
        guard let url = URL(string: value), url.scheme?.lowercased() == "https" else {
            throw WatchdogError.invalidDownloadURL(value)
        }
        return url
    }

    private static func requiredPID(in arguments: [String]) throws -> pid_t {
        let value = try requiredArgument(parentPIDArgument, in: arguments)
        guard let pid = pid_t(value), pid > 0 else {
            throw WatchdogError.invalidPID(value)
        }
        return pid
    }

    private static func requiredDeadline(in arguments: [String]) throws -> UpdateInstallDeadline {
        let value = try requiredArgument(deadlineArgument, in: arguments)
        guard let deadline = UInt64(value) else {
            throw WatchdogError.invalidDeadline(value)
        }
        return UpdateInstallDeadline(deadlineUptimeNanoseconds: deadline)
    }

    private static func requiredContentLength(in arguments: [String]) throws -> UInt64 {
        let value = try requiredArgument(contentLengthArgument, in: arguments)
        guard let contentLength = UInt64(value), contentLength > 0 else {
            throw WatchdogError.invalidContentLength(value)
        }
        return contentLength
    }

    private static func showFallbackNotice(expectedVersion: String) {
        let body = "自动安装在 60 秒内未完成，已打开更新磁盘映像。请将其中的 Codex Mixin.app 拖入 Applications（目标版本：\(expectedVersion)）。"
        showNotice(title: "已切换为手动安装", body: body)
    }

    private static func showFallbackFailure(_ error: Error) {
        let response = showNotice(
            title: "自动更新失败",
            body: "自动安装未完成，且无法打开或校验 DMG。请点击“打开 Release 页面”，下载后将 Codex Mixin.app 拖入 Applications。\n\n错误：\(error)",
            actionTitle: "打开 Release 页面"
        )
        if response == .alertFirstButtonReturn,
           let releaseURL = URL(
               string: "https://github.com/Edward-lyz/codex-mixin/releases/latest"
           )
        {
            NSWorkspace.shared.open(releaseURL)
        }
    }

    @discardableResult
    private static func showNotice(
        title: String,
        body: String,
        actionTitle: String? = nil
    ) -> NSApplication.ModalResponse {
        let present = {
            let application = NSApplication.shared
            application.setActivationPolicy(.accessory)
            application.activate(ignoringOtherApps: true)
            let alert = NSAlert()
            alert.messageText = title
            alert.informativeText = body
            alert.alertStyle = .warning
            if let actionTitle {
                alert.addButton(withTitle: actionTitle)
            }
            alert.addButton(withTitle: "好")
            alert.window.level = .floating
            let response = alert.runModal()
            alert.window.close()
            return response
        }
        if Thread.isMainThread {
            return present()
        } else {
            return DispatchQueue.main.sync(execute: present)
        }
    }

    private enum WatchdogError: Error, CustomStringConvertible {
        case timedOut
        case missingArgument(String)
        case invalidPID(String)
        case invalidDownloadURL(String)
        case invalidDeadline(String)
        case invalidContentLength(String)
        case fallbackFileQuarantineFailed(String, String)
        case fallbackDownloadFailed(Int32, stderr: String)
        case fallbackDownloadSizeMismatch(expected: UInt64, actual: UInt64?)
        case fallbackSignatureInvalid
        case fallbackOpenFailed(String)

        var description: String {
            switch self {
            case .timedOut:
                return "automatic update exceeded its deadline"
            case .missingArgument(let argument):
                return "missing \(argument)"
            case .invalidPID(let value):
                return "invalid parent PID \(value)"
            case .invalidDownloadURL(let value):
                return "fallback download URL must use HTTPS: \(value)"
            case .invalidDeadline(let value):
                return "invalid update deadline \(value)"
            case .invalidContentLength(let value):
                return "invalid fallback content length \(value)"
            case .fallbackFileQuarantineFailed(let path, let reason):
                return "could not quarantine stale fallback DMG \(path): \(reason)"
            case .fallbackDownloadFailed(let status, let stderr):
                return "fallback DMG download failed with exit status \(status); stderr=\(stderr.isEmpty ? "<empty>" : stderr)"
            case .fallbackDownloadSizeMismatch(let expected, let actual):
                return "fallback DMG size mismatch: expected \(expected) bytes, got \(actual.map(String.init) ?? "missing")"
            case .fallbackSignatureInvalid:
                return "fallback DMG failed Sparkle Ed25519 signature verification"
            case .fallbackOpenFailed(let path):
                return "Finder could not open fallback DMG \(path)"
            }
        }
    }
}
