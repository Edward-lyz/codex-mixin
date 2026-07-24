import Cocoa

extension AppDelegate {
    @objc func startService() {
        serviceStatus = "本地网关启动中..."
        serviceEndpoint = nil
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                let status = try await ensureGatewayReady()
                applyGatewayStatus(status)
                await refreshStatusNow()
            } catch {
                isRunning = false
                serviceStatus = "本地网关启动失败"
                serviceEndpoint = nil
                updateStatusTitle()
                showAlert(title: "启动服务失败", message: String(describing: error))
            }
        }
    }

    @objc func restartService() {
        serviceStatus = "本地网关重启中..."
        serviceEndpoint = nil
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await restartGatewayProcess()
                let status = try await waitForGatewayStatus()
                applyGatewayStatus(status)
                await refreshStatusNow()
            } catch {
                isRunning = false
                serviceStatus = "本地网关重启失败"
                serviceEndpoint = nil
                updateStatusTitle()
                showAlert(title: "重启服务失败", message: String(describing: error))
            }
        }
    }

    @objc func toggleLaunchAtLogin() {
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                if FileManager.default.fileExists(atPath: launchAgentPath().path) {
                    let statusBefore = try? await runGateway(["status"])
                    let wasRunning = statusBefore?.contains("gateway: running") == true
                    try await bootoutIfLoaded(launchDomainAndLabel())
                    try await bootoutIfLoaded(menuLaunchDomainAndLabel())
                    try FileManager.default.removeItem(at: launchAgentPath())
                    if FileManager.default.fileExists(atPath: menuLaunchAgentPath().path) {
                        try FileManager.default.removeItem(at: menuLaunchAgentPath())
                    }
                    if wasRunning && statusBefore?.contains("daemon: running") != true {
                        try await waitForGatewayStopped()
                        _ = try await runGateway(["start", "--daemon"])
                        _ = try await waitForGatewayStatus()
                    }
                } else {
                    _ = try await runGateway(["config", "--json", "--scope", "effective"])
                    try await bootoutIfLoaded(launchDomainAndLabel())
                    _ = try await runGateway(["stop"])
                    try await waitForGatewayStopped()
                    try installLaunchAgent()
                    _ = try await runProcess("/bin/launchctl", ["bootstrap", launchDomain(), launchAgentPath().path])
                    _ = try await waitForGatewayStatus()
                }
                await refreshStatusNow()
            } catch {
                showAlert(title: "更新登录自启失败", message: String(describing: error))
            }
        }
    }

    @objc func refreshStatus() {
        Task { @MainActor in
            await refreshStatusNow()
        }
    }

    func startGatewayAtLaunch() {
        serviceStatus = "本地网关启动中..."
        serviceEndpoint = nil
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                do {
                    _ = try await runGateway(["config", "--json", "--scope", "effective"])
                } catch {
                    guard isMissingGatewayConfiguration(error) else { throw error }
                    isRunning = false
                    serviceStatus = "等待配置上游 API"
                    serviceEndpoint = nil
                    updateQuotaStatus(title: "额度：等待配置", detail: nil, progress: nil)
                    updateStatusTitle()
                    updateActionStates()
                    if !CommandLine.arguments.contains("--show-settings")
                        && !CommandLine.arguments.contains("--check-updates")
                    {
                        DispatchQueue.main.async { [weak self] in
                            self?.configureLogin()
                        }
                    }
                    return
                }
                if FileManager.default.fileExists(atPath: launchAgentPath().path) {
                    try installMenuLaunchAgent()
                }
                let status = try await ensureGatewayReady()
                applyGatewayStatus(status)
                do {
                    _ = try await runGateway(["refresh-codex-catalog"])
                } catch {
                    showAlert(title: "刷新 Codex 模型失败", message: String(describing: error))
                }
                await refreshStatusNow()
            } catch {
                isRunning = false
                serviceStatus = "本地网关启动失败"
                serviceEndpoint = nil
                updateStatusTitle()
                if !CommandLine.arguments.contains("--show-settings") {
                    showAlert(title: "自动启动网关失败", message: String(describing: error))
                }
            }
        }
    }

    func ensureGatewayReady() async throws -> String {
        await initializeProviderModelsIfNeeded()
        if let status = try? await runGateway(["status"]), status.contains("gateway: running") {
            let launchAgentInstalled = FileManager.default.fileExists(atPath: launchAgentPath().path)
            var launchAgentNeedsMigration = false
            if launchAgentInstalled {
                launchAgentNeedsMigration = try launchAgentNeedsUpdate()
            }
            let gatewayVersion = status
                .split(separator: "\n")
                .first(where: { $0.hasPrefix("gateway-version: ") })
                .map { String($0.dropFirst("gateway-version: ".count)) }
            if gatewayVersion != appVersion()
                || (launchAgentInstalled
                    && (status.contains("daemon: running") || launchAgentNeedsMigration)) {
                try await restartGatewayProcess()
                return try await waitForGatewayStatus()
            }
            return status
        }
        _ = try await runGateway(["config", "--json", "--scope", "effective"])
        if FileManager.default.fileExists(atPath: launchAgentPath().path) {
            if (try? await runProcess("/bin/launchctl", ["print", launchDomainAndLabel()])) != nil,
                let status = try? await waitForGatewayStatus()
            {
                return status
            }
            try await bootoutIfLoaded(launchDomainAndLabel())
            try await waitForGatewayStopped()
            try installLaunchAgent()
            _ = try await runProcess("/bin/launchctl", ["bootstrap", launchDomain(), launchAgentPath().path])
        } else {
            _ = try await runGateway(["start", "--daemon"])
        }
        return try await waitForGatewayStatus()
    }

    func initializeProviderModelsIfNeeded() async {
        do {
            let response = try decodeProviderList(
                try await runGateway(["providers", "list", "--json"])
            )
            for provider in response.providers
                where provider.enabled
                    && provider.modelsRefreshedAtMilliseconds == nil
                    && provider.cachedModels.isEmpty
            {
                serviceStatus = "正在迁移 \(provider.displayName) 模型配置..."
                do {
                    _ = try await runGateway(["providers", "discover", provider.id])
                } catch {
                    appendDiagnosticLog(
                        "Initial model discovery failed for \(provider.id)\n"
                            + localizedErrorDescription(error)
                    )
                }
            }
        } catch {
            appendDiagnosticLog(
                "Initial Provider migration check failed\n" + localizedErrorDescription(error)
            )
        }
    }

    func isMissingGatewayConfiguration(_ error: Error) -> Bool {
        let message = String(describing: error)
        return message.contains("provider configuration is missing")
            || message.contains("provider configuration is empty")
    }

    func restartGatewayProcess() async throws {
        if FileManager.default.fileExists(atPath: launchAgentPath().path) {
            try await bootoutIfLoaded(launchDomainAndLabel())
            _ = try await runGateway(["stop"])
            try await waitForGatewayStopped()
            try installLaunchAgent()
            _ = try await runProcess("/bin/launchctl", ["bootstrap", launchDomain(), launchAgentPath().path])
            return
        }
        _ = try await runGateway(["stop"])
        try await waitForGatewayStopped()
        _ = try await runGateway(["start", "--daemon"])
    }

    func waitForGatewayStatus() async throws -> String {
        var lastError = "网关尚未报告健康状态"
        for _ in 0..<20 {
            do {
                let status = try await runGateway(["status"])
                if status.contains("gateway: running") {
                    return status
                }
                lastError = status
            } catch {
                lastError = String(describing: error)
            }
            try await Task.sleep(nanoseconds: 250_000_000)
        }
        throw GatewayError.command("网关启动后 5 秒内未就绪：\(lastError)")
    }

    func waitForGatewayStopped() async throws {
        let runtimeURL = stateDir().appendingPathComponent("runtime.json")
        for _ in 0..<20 {
            guard FileManager.default.fileExists(atPath: runtimeURL.path) else {
                return
            }
            let data = try Data(contentsOf: runtimeURL)
            guard
                let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                let pid = object["pid"] as? NSNumber
            else {
                throw GatewayError.command("无法读取网关 runtime PID：\(runtimeURL.path)")
            }
            if kill(pid.int32Value, 0) != 0 {
                let errorCode = errno
                if errorCode == ESRCH {
                    return
                }
                if errorCode != EPERM {
                    throw GatewayError.command("检查网关进程 \(pid) 失败：errno \(errorCode)")
                }
            }
            try await Task.sleep(nanoseconds: 250_000_000)
        }
        throw GatewayError.command("网关在 5 秒内未停止，可能存在不受 Codex Mixin 管理的进程。")
    }

    func applyGatewayStatus(_ status: String?) {
        isRunning = status?.contains("gateway: running") == true
        let providerReadiness = status?
            .split(separator: "\n")
            .first(where: { $0.hasPrefix("provider-readiness: ") })
            .map { String($0.dropFirst("provider-readiness: ".count)) }
        serviceEndpoint = status?
            .split(separator: "\n")
            .first(where: { $0.hasPrefix("endpoint: ") })
            .map { String($0.dropFirst("endpoint: ".count)) }
        if isRunning, providerReadiness == "degraded" {
            serviceStatus = "本地网关运行中 · Provider 降级"
        } else if isRunning, providerReadiness == "disabled" {
            serviceStatus = "本地网关运行中 · 无启用 Provider"
        } else {
            serviceStatus = isRunning ? "本地网关运行中" : "本地网关已停止"
        }
        updateStatusTitle()
        updateActionStates()
    }

    func refreshStatusNow() async {
        do {
            applyGatewayStatus(try await runGateway(["status"]))
        } catch {
            let message = String(describing: error)
            let missingConfiguration = isMissingGatewayConfiguration(error)
            isRunning = false
            serviceEndpoint = nil
            if missingConfiguration {
                serviceStatus = "等待配置上游 API"
            } else if message.contains("gateway not running") {
                serviceStatus = "本地网关已停止"
            } else {
                serviceStatus = "网关状态检查失败"
            }
            updateStatusTitle()
            updateActionStates()
            if missingConfiguration {
                updateQuotaStatus(title: "额度：等待配置", detail: nil, progress: nil)
                return
            }
        }
        do {
            let quota = try await runGateway(["quota", "--json"])
            updateProviderQuotaStatus(try parseProviderQuotaUsage(quota))
        } catch {
            updateQuotaStatus(
                title: "Provider 额度：不可用",
                detail: localizedErrorDescription(error),
                progress: nil
            )
        }
    }

    func updateQuotaStatus(title: String, detail: String?, progress: Double?) {
        quotaStatusItem?.view = quotaMenuView(title: title, detail: detail, progress: progress)
    }

    func updateProviderQuotaStatus(_ usages: [ProviderQuotaUsage]) {
        quotaStatusItem?.view = providerQuotaMenuView(usages)
    }
    @objc func openLogs() {
        let logURL = stateDir().appendingPathComponent("gateway.log")
        if !FileManager.default.fileExists(atPath: logURL.path) {
            showAlert(title: "日志还不存在", message: "本地网关启动后会写入 \(logURL.path)。")
            return
        }
        NSWorkspace.shared.open(logURL)
    }

    @objc func openConfigFolder() {
        do {
            try FileManager.default.createDirectory(at: stateDir(), withIntermediateDirectories: true)
            NSWorkspace.shared.open(stateDir())
        } catch {
            showAlert(title: "打开配置目录失败", message: String(describing: error))
        }
    }

    @objc func quit() {
        NSApp.terminate(nil)
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if terminationInProgress {
            return .terminateLater
        }
        terminationInProgress = true
        serviceBusy = true
        serviceStatus = "正在停止本地网关..."
        serviceEndpoint = nil
        Task { @MainActor in
            do {
                try await bootoutIfLoaded(launchDomainAndLabel())
                _ = try await runGateway(["stop"])
                try await waitForGatewayStopped()
                timer?.invalidate()
                sender.reply(toApplicationShouldTerminate: true)
            } catch {
                terminationInProgress = false
                serviceBusy = false
                await refreshStatusNow()
                showAlert(title: "退出 Codex Mixin 失败", message: "本地网关未能停止：\(error)")
                sender.reply(toApplicationShouldTerminate: false)
            }
        }
        return .terminateLater
    }

    func installLaunchAgent() throws {
        try FileManager.default.createDirectory(at: stateDir(), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: launchAgentPath().deletingLastPathComponent(), withIntermediateDirectories: true)

        let executable = try gatewayExecutableURL()
        let logFile = stateDir().appendingPathComponent("gateway.log").path
        let plist = """
        <?xml version="1.0" encoding="UTF-8"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
        <plist version="1.0">
        <dict>
          <key>Label</key>
          <string>\(serviceLabel)</string>
          <key>ProgramArguments</key>
          <array>
            <string>\(xmlEscape(executable.path))</string>
            <string>start</string>
            <string>--log-file</string>
            <string>\(xmlEscape(logFile))</string>
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
          <string>Background</string>
          <key>StandardOutPath</key>
          <string>/dev/null</string>
          <key>StandardErrorPath</key>
          <string>/dev/null</string>
          <key>WorkingDirectory</key>
          <string>\(xmlEscape(FileManager.default.homeDirectoryForCurrentUser.path))</string>
        </dict>
        </plist>
        """
        try plist.write(to: launchAgentPath(), atomically: true, encoding: .utf8)
        try installMenuLaunchAgent()
    }

    func installMenuLaunchAgent() throws {
        try FileManager.default.createDirectory(at: menuLaunchAgentPath().deletingLastPathComponent(), withIntermediateDirectories: true)
        let plist = """
        <?xml version="1.0" encoding="UTF-8"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
        <plist version="1.0">
        <dict>
          <key>Label</key>
          <string>\(menuLaunchLabel)</string>
          <key>ProgramArguments</key>
          <array>
            <string>/usr/bin/open</string>
            <string>-g</string>
            <string>\(xmlEscape(Bundle.main.bundleURL.path))</string>
          </array>
          <key>RunAtLoad</key>
          <true/>
          <key>ProcessType</key>
          <string>Interactive</string>
          <key>StandardOutPath</key>
          <string>/dev/null</string>
          <key>StandardErrorPath</key>
          <string>/dev/null</string>
        </dict>
        </plist>
        """
        try plist.write(to: menuLaunchAgentPath(), atomically: true, encoding: .utf8)
    }

    func launchAgentNeedsUpdate() throws -> Bool {
        let data = try Data(contentsOf: launchAgentPath())
        guard
            let plist = try PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
            let arguments = plist["ProgramArguments"] as? [String],
            let keepAlive = plist["KeepAlive"] as? [String: Any]
        else {
            return true
        }
        let expectedArguments = [
            try gatewayExecutableURL().path,
            "start",
            "--log-file",
            stateDir().appendingPathComponent("gateway.log").path,
        ]
        return arguments != expectedArguments
            || plist["RunAtLoad"] as? Bool != true
            || keepAlive["SuccessfulExit"] as? Bool != false
            || plist["ThrottleInterval"] as? Int != 10
            || plist["ProcessType"] as? String != "Background"
    }

    func runGateway(_ arguments: [String]) async throws -> String {
        do {
            return try await runProcess(try gatewayExecutableURL().path, arguments)
        } catch {
            let action = arguments.prefix(2).joined(separator: " ")
            appendDiagnosticLog(
                "App CLI operation failed: \(action.isEmpty ? "<default>" : action)\n\(String(describing: error))"
            )
            throw error
        }
    }

    func bootoutIfLoaded(_ domainAndLabel: String) async throws {
        do {
            _ = try await runProcess("/bin/launchctl", ["bootout", domainAndLabel])
        } catch {
            let message = String(describing: error)
            if !message.contains("No such process") && !message.contains("Could not find service") {
                throw error
            }
        }
    }

    func runProcess(_ executable: String, _ arguments: [String]) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                let process = Process()
                let outputPipe = Pipe()
                process.executableURL = URL(fileURLWithPath: executable)
                process.arguments = arguments
                process.standardOutput = outputPipe
                process.standardError = outputPipe
                var environment = ProcessInfo.processInfo.environment
                let ignoredKeys = environment.keys.filter { key in
                    key.hasPrefix("CODEX_GATEWAY_")
                    || key == "ANTHROPIC_BASE_URL"
                    || key == "ANTHROPIC_API_KEY"
                }
                for key in ignoredKeys {
                    environment.removeValue(forKey: key)
                }
                process.environment = environment
                do {
                    try process.run()
                    process.waitUntilExit()
                    let data = outputPipe.fileHandleForReading.readDataToEndOfFile()
                    let output = String(data: data, encoding: .utf8) ?? ""
                    let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
                    if process.terminationStatus == 0 {
                        continuation.resume(returning: trimmed)
                    } else {
                        continuation.resume(throwing: GatewayError.command(trimmed.isEmpty ? "exit \(process.terminationStatus)" : trimmed))
                    }
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    func gatewayExecutableURL() throws -> URL {
        if let resourceURL = Bundle.main.resourceURL {
            let bundled = resourceURL.appendingPathComponent("codex-mixin")
            if FileManager.default.isExecutableFile(atPath: bundled.path) {
                return bundled
            }
        }
        throw GatewayError.command("bundled codex-mixin executable not found")
    }

    func stateDir() -> URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".codex-mixin")
    }

    func appendDiagnosticLog(_ message: String) {
        let directory = stateDir()
        let logURL = directory.appendingPathComponent("gateway.log")
        do {
            try FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
            if !FileManager.default.fileExists(atPath: logURL.path) {
                FileManager.default.createFile(
                    atPath: logURL.path,
                    contents: nil,
                    attributes: [.posixPermissions: NSNumber(value: 0o600)]
                )
            }
            let formatter = ISO8601DateFormatter()
            let boundedMessage = String(message.prefix(8_000))
            let entry = "\n\(formatter.string(from: Date())) APP_DIAGNOSTIC \(boundedMessage)\n"
            let handle = try FileHandle(forWritingTo: logURL)
            try handle.seekToEnd()
            if let data = entry.data(using: .utf8) {
                try handle.write(contentsOf: data)
            }
            try handle.close()
        } catch {
            NSLog("Codex Mixin could not append diagnostic log: \(error)")
        }
    }

    func launchAgentPath() -> URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents")
            .appendingPathComponent("\(serviceLabel).plist")
    }

    func menuLaunchAgentPath() -> URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents")
            .appendingPathComponent("\(menuLaunchLabel).plist")
    }

    func launchDomain() -> String {
        "gui/\(getuid())"
    }

    func launchDomainAndLabel() -> String {
        "\(launchDomain())/\(serviceLabel)"
    }

    func menuLaunchDomainAndLabel() -> String {
        "\(launchDomain())/\(menuLaunchLabel)"
    }
}
