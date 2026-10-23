import Cocoa
import Sparkle

extension AppDelegate: SPUUpdaterDelegate {
    func appVersion() -> String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
    }

    @MainActor
    @objc func checkForUpdatesFromMenu() {
        guard let updater, !updater.sessionInProgress else { return }
        updateDriver?.silent = false
        updater.checkForUpdates()
    }

    func startUpdater() {
        let driver = SilentUpdateDriver(hostBundle: .main, delegate: nil)
        updateDriver = driver
        let updater = SPUUpdater(hostBundle: .main, applicationBundle: .main, userDriver: driver, delegate: self)
        self.updater = updater
        updater.automaticallyChecksForUpdates = false
        // The driver explicitly downloads and waits. Sparkle's on-quit install
        // path would bypass the gateway's idle gate.
        updater.automaticallyDownloadsUpdates = false
        do { try updater.start() } catch {
            appendDiagnosticLog("APP_UPDATE start failed: \(diagnosticErrorDescription(error))")
            return
        }
        if let saved = UserDefaults.standard.data(forKey: "silentUpdatePolicy") {
            do { silentUpdatePolicy = try JSONDecoder().decode(SilentUpdatePolicy.self, from: saved) }
            catch { appendDiagnosticLog("APP_UPDATE invalid retry state: \(diagnosticErrorDescription(error))") }
        }
        automaticUpdateTask = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                await self?.tickAutomaticUpdate()
                do { try await Task.sleep(nanoseconds: 60_000_000_000) } catch { return }
            }
        }
    }

    private func tickAutomaticUpdate() async {
        guard let updater, let driver = updateDriver, !serviceBusy else { return }
        if let reply = driver.pendingInstall {
            do {
                let output = try await runGateway(["status", "--json"])
                guard let bytes = output.data(using: .utf8),
                      let status = try JSONSerialization.jsonObject(with: bytes) as? [String: Any],
                      silentUpdatePolicy.canInstall(status: status),
                      !Task.isCancelled, !serviceBusy
                else { return }
                driver.pendingInstall = nil
                UserDefaults.standard.set(true, forKey: "preserveGatewayOnUpdateRelaunch")
                reply(.install)
            } catch {
                appendDiagnosticLog("APP_UPDATE idle check failed: \(diagnosticErrorDescription(error))")
            }
            return
        }
        guard !updater.sessionInProgress, Date() >= nextAutomaticUpdateCheck else { return }
        nextAutomaticUpdateCheck = Date().addingTimeInterval(SilentUpdatePolicy.checkInterval)
        do {
            let output = try await runGateway(["update", "--check", "--json"])
            guard let bytes = output.data(using: .utf8),
                  let report = try JSONSerialization.jsonObject(with: bytes) as? [String: Any],
                  let latest = report["latest_version"] as? String,
                  latest != appVersion(), isInstalledVersionAtLeast(latest, expectedVersion: appVersion()),
                  !Task.isCancelled
            else { return }
            silentUpdatePolicy.select(latest)
            guard silentUpdatePolicy.beginAttempt(now: Date()) else { return }
            UserDefaults.standard.set(try JSONEncoder().encode(silentUpdatePolicy), forKey: "silentUpdatePolicy")
            // Skip non-writable installations instead of invoking authorization UI.
            guard FileManager.default.isWritableFile(atPath: Bundle.main.bundlePath),
                  FileManager.default.isWritableFile(atPath: Bundle.main.bundleURL.deletingLastPathComponent().path)
            else { return }
            driver.silent = true
            updater.checkForUpdatesInBackground()
        } catch {
            appendDiagnosticLog("APP_UPDATE automatic check failed: \(diagnosticErrorDescription(error))")
        }
    }

    @MainActor
    func updater(
        _ updater: SPUUpdater,
        userDidMake choice: SPUUserUpdateChoice,
        forUpdate updateItem: SUAppcastItem,
        state: SPUUserUpdateState
    ) {
        guard choice == .install, updateDriver?.silent != true else { return }
        startUpdateWatchdog(for: updateItem)
    }

    @MainActor
    func updater(_ updater: SPUUpdater, willInstallUpdate updateItem: SUAppcastItem) {
        guard updateDriver?.silent != true else { return }
        startUpdateWatchdog(for: updateItem)
    }

    @MainActor
    func updater(_ updater: SPUUpdater, didAbortWithError error: Error) {
        UserDefaults.standard.removeObject(forKey: "preserveGatewayOnUpdateRelaunch")
        updateDriver?.cancelPendingInstall()
        appendAppDiagnosticLog(
            "APP_UPDATE Sparkle aborted error=\(diagnosticErrorDescription(error))",
            directory: stateDir()
        )
    }

    @MainActor
    func updater(
        _ updater: SPUUpdater,
        didFinishUpdateCycleFor updateCheck: SPUUpdateCheck,
        error: Error?
    ) {
        guard let error else { return }
        appendAppDiagnosticLog(
            "APP_UPDATE Sparkle finished check=\(updateCheck.rawValue) error=\(diagnosticErrorDescription(error))",
            directory: stateDir()
        )
    }

    @MainActor
    private func startUpdateWatchdog(for updateItem: SUAppcastItem) {
        guard let downloadURL = updateItem.fileURL else {
            appendAppDiagnosticLog(
                "APP_UPDATE watchdog not started: Sparkle update has no file URL version=\(updateItem.versionString)",
                directory: stateDir()
            )
            return
        }
        guard let enclosure = updateItem.propertiesDictionary["enclosure"] as? [String: Any],
              let edSignature = enclosure["sparkle:edSignature"] as? String,
              !edSignature.isEmpty
        else {
            appendAppDiagnosticLog(
                "APP_UPDATE watchdog not started: Sparkle update has no Ed25519 signature version=\(updateItem.versionString)",
                directory: stateDir()
            )
            return
        }

        let deadline = UpdateInstallDeadline()
        let request = UpdateWatchdogLaunchRequest(
            downloadURL: downloadURL,
            contentLength: updateItem.contentLength,
            edSignature: edSignature,
            expectedVersion: updateItem.versionString,
            destinationURL: Bundle.main.bundleURL,
            parentPID: ProcessInfo.processInfo.processIdentifier,
            deadlineUptimeNanoseconds: deadline.deadlineUptimeNanoseconds
        )
        guard updateWatchdogLaunchGate.reserve(request) else {
            appendAppDiagnosticLog(
                "APP_UPDATE watchdog already started version=\(request.expectedVersion) url=\(request.downloadURL.absoluteString)",
                directory: stateDir()
            )
            return
        }

        do {
            let executableURL = try copyWatchdogExecutable()
            let watchdog = Process()
            watchdog.executableURL = executableURL
            watchdog.arguments = request.arguments
            watchdog.standardOutput = FileHandle.nullDevice
            watchdog.standardError = FileHandle.nullDevice
            try watchdog.run()
            appendAppDiagnosticLog(
                "APP_UPDATE watchdog started pid=\(watchdog.processIdentifier) version=\(request.expectedVersion) deadline_uptime_ns=\(request.deadlineUptimeNanoseconds)",
                directory: stateDir()
            )
        } catch {
            updateWatchdogLaunchGate.release(request)
            appendAppDiagnosticLog(
                "APP_UPDATE watchdog launch failed \(diagnosticErrorDescription(error))",
                directory: stateDir()
            )
        }
    }

    private func copyWatchdogExecutable() throws -> URL {
        let fileManager = FileManager.default
        guard let sourceExecutable = Bundle.main.executableURL else {
            throw UpdateWatchdogLaunchError.missingExecutable
        }

        let sourceContents = Bundle.main.bundleURL.appendingPathComponent("Contents")
        let sourceFramework = sourceContents.appendingPathComponent("Frameworks/Sparkle.framework")
        guard fileManager.fileExists(atPath: sourceFramework.path) else {
            throw UpdateWatchdogLaunchError.missingFramework
        }

        let root = fileManager.temporaryDirectory
            .appendingPathComponent("CodexMixin-update-watchdog-\(UUID().uuidString)", isDirectory: true)
        let contents = root.appendingPathComponent("Watchdog.app/Contents", isDirectory: true)
        let executable = contents.appendingPathComponent("MacOS/CodexMixinMenu")
        try fileManager.createDirectory(
            at: executable.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try fileManager.copyItem(at: sourceExecutable, to: executable)

        let sourceInfo = sourceContents.appendingPathComponent("Info.plist")
        if fileManager.fileExists(atPath: sourceInfo.path) {
            try fileManager.copyItem(
                at: sourceInfo,
                to: contents.appendingPathComponent("Info.plist")
            )
        }

        let frameworks = contents.appendingPathComponent("Frameworks", isDirectory: true)
        try fileManager.createDirectory(at: frameworks, withIntermediateDirectories: true)
        try fileManager.copyItem(
            at: sourceFramework,
            to: frameworks.appendingPathComponent("Sparkle.framework")
        )
        return executable
    }
}

private enum UpdateWatchdogLaunchError: Error, CustomStringConvertible {
    case missingExecutable
    case missingFramework

    var description: String {
        switch self {
        case .missingExecutable:
            return "main application executable is unavailable"
        case .missingFramework:
            return "Sparkle.framework is unavailable in the application bundle"
        }
    }
}
