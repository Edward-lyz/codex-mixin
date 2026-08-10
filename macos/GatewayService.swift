import Cocoa

private struct GatewayHealthResponse: Decodable {
    let ok: Bool
}

extension AppDelegate {
    @objc func startService() {
        serviceStatus = "本地网关启动中..."
        serviceEndpoint = nil
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在启动本地网关",
                    phases: [
                        "准备配置",
                        "启动网关",
                        "等待就绪",
                    ],
                    successTitle: "✓ 网关已启动",
                    failureTitle: "✗ 启动失败",
                    showFailureAlert: true,
                    failureAlertTitle: "启动服务失败"
                ) { progress in
                    progress.advance(to: 0)
                    progress.advance(to: 1)
                    let status = try await ensureGatewayReady()
                    progress.advance(to: 2)
                    applyGatewayStatus(status)
                    await refreshStatusNow()
                }
            } catch {
                isRunning = false
                serviceStatus = "本地网关启动失败"
                serviceEndpoint = nil
                updateStatusTitle()
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
                try await runOperationProgress(
                    title: "正在重启本地网关",
                    phases: [
                        "停止旧进程",
                        "启动网关",
                        "等待就绪",
                    ],
                    successTitle: "✓ 网关已重启",
                    failureTitle: "✗ 重启失败",
                    showFailureAlert: true,
                    failureAlertTitle: "重启服务失败"
                ) { progress in
                    progress.advance(to: 0)
                    try await restartGatewayProcess()
                    progress.advance(to: 1)
                    let status = try await waitForGatewayStatus()
                    progress.advance(to: 2)
                    applyGatewayStatus(status)
                    await refreshStatusNow()
                }
            } catch {
                isRunning = false
                serviceStatus = "本地网关重启失败"
                serviceEndpoint = nil
                updateStatusTitle()
            }
        }
    }

    @objc func stopService() {
        serviceStatus = "本地网关停止中..."
        serviceEndpoint = nil
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在停止本地网关",
                    phases: [
                        "停止网关进程",
                        "等待退出",
                        "完成",
                    ],
                    successTitle: "✓ 网关已停止",
                    failureTitle: "✗ 停止失败",
                    showFailureAlert: true,
                    failureAlertTitle: "停止服务失败"
                ) { progress in
                    progress.advance(to: 0)
                    try await bootoutIfLoaded(launchDomainAndLabel())
                    _ = try? await runGateway(["stop"])
                    progress.advance(to: 1)
                    try await waitForGatewayStopped()
                    progress.advance(to: 2)
                    isRunning = false
                    providerStatusDetail = nil
                    serviceStatus = "本地网关已停止"
                    updateStatusTitle()
                }
            } catch {
                await refreshStatusNow()
            }
        }
    }

    @objc func toggleLaunchAtLogin() {
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在更新登录自启",
                    phases: [
                        "更新 LaunchAgent",
                        "应用网关状态",
                        "完成",
                    ],
                    successTitle: "✓ 登录自启已更新",
                    failureTitle: "✗ 更新失败",
                    showFailureAlert: true,
                    failureAlertTitle: "更新登录自启失败"
                ) { progress in
                    progress.advance(to: 0)
                    if FileManager.default.fileExists(atPath: launchAgentPath().path) {
                        let statusBefore = try? await runGateway(["status"])
                        let wasRunning = statusBefore?.contains("gateway: running") == true
                        try await bootoutIfLoaded(launchDomainAndLabel())
                        try await bootoutIfLoaded(menuLaunchDomainAndLabel())
                        try FileManager.default.removeItem(at: launchAgentPath())
                        if FileManager.default.fileExists(atPath: menuLaunchAgentPath().path) {
                            try FileManager.default.removeItem(at: menuLaunchAgentPath())
                        }
                        progress.advance(to: 1)
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
                        progress.advance(to: 1)
                        _ = try await runProcess(
                            "/bin/launchctl",
                            ["bootstrap", launchDomain(), launchAgentPath().path]
                        )
                        _ = try await waitForGatewayStatus()
                    }
                    progress.advance(to: 2)
                    await refreshStatusNow()
                }
            } catch {
                // Failure already shown by the progress window + alert.
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
                    updateTokenUsageStatus(title: "Token 使用：等待配置", detail: nil, progress: nil)
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
        let providerIssues = providerIssueDetails(fromGatewayStatus: status)
        providerStatusDetail = providerIssues.isEmpty
            ? nil
            : providerIssues.joined(separator: "；")
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
        await requestStatusRefresh(scope: .full)
    }

    func refreshMenuStatus() async {
        await requestStatusRefresh(scope: .status)
    }

    func refreshScheduledStatus() async {
        let scope: StatusRefreshScope = quotaRefreshPolicy.isDue() ? .status : .health
        await requestStatusRefresh(scope: scope)
    }

    private func requestStatusRefresh(scope: StatusRefreshScope) async {
        pendingStatusRefreshScope = pendingStatusRefreshScope?.merged(with: scope) ?? scope
        await statusRefreshCoordinator.refresh()
    }

    func performStatusRefresh(
        isCurrent: @escaping StatusRefreshCoordinator.IsCurrent
    ) async {
        let scope = pendingStatusRefreshScope ?? .full
        pendingStatusRefreshScope = nil
        if scope == .health {
            do {
                try await checkGatewayHealth()
                guard await isCurrent() else { return }
                applyHealthyGatewaySnapshot()
            } catch {
                do {
                    let status = try await runGateway(["status"])
                    guard await isCurrent() else { return }
                    applyGatewayStatus(status)
                } catch {
                    guard await isCurrent() else { return }
                    _ = applyGatewayStatusFailure(error)
                }
            }
            return
        }

        do {
            let status = try await runGateway(["status"])
            guard await isCurrent() else { return }
            applyGatewayStatus(status)
        } catch {
            guard await isCurrent() else { return }
            if applyGatewayStatusFailure(error) {
                return
            }
        }
        guard scope == .full || quotaRefreshPolicy.isDue() else { return }
        quotaRefreshPolicy.markAttempt()
        // Quota pages can involve several remote providers and browser-backed
        // dashboards. Start both subprocesses together so a slow quota probe
        // cannot hide the local token history on a fresh app launch.
        let quotaTask = Task { try await runGateway(["quota", "--json"]) }
        let usageTask = Task { try await runGateway(["usage", "--json"]) }
        do {
            let usage = try await usageTask.value
            guard await isCurrent() else { return }
            updateProviderTokenUsageStatus(try parseProviderTokenUsage(usage))
        } catch {
            guard await isCurrent() else { return }
            updateTokenUsageStatus(
                title: "Token 使用：不可用",
                detail: localizedErrorDescription(error),
                progress: nil
            )
        }
        do {
            let quota = try await quotaTask.value
            guard await isCurrent() else { return }
            updateProviderQuotaStatus(try parseProviderQuotaUsage(quota))
        } catch {
            guard await isCurrent() else { return }
            updateQuotaStatus(
                title: "Provider 额度：不可用",
                detail: localizedErrorDescription(error),
                progress: nil
            )
        }
    }

    private func checkGatewayHealth() async throws {
        guard
            let serviceEndpoint,
            var healthURL = URL(string: serviceEndpoint)
        else {
            throw GatewayError.command("gateway endpoint is unavailable")
        }
        if healthURL.lastPathComponent == "v1" {
            healthURL.deleteLastPathComponent()
        }
        healthURL.appendPathComponent("healthz")
        var request = URLRequest(url: healthURL)
        request.timeoutInterval = 2
        let (data, response) = try await URLSession.shared.data(for: request)
        guard
            let httpResponse = response as? HTTPURLResponse,
            (200 ... 299).contains(httpResponse.statusCode),
            try JSONDecoder().decode(GatewayHealthResponse.self, from: data).ok
        else {
            throw GatewayError.command("gateway health check failed")
        }
    }

    private func applyHealthyGatewaySnapshot() {
        isRunning = true
        if providerStatusDetail != nil {
            serviceStatus = "本地网关运行中 · Provider 降级"
        } else if !serviceStatus.contains("无启用 Provider") {
            serviceStatus = "本地网关运行中"
        }
        updateStatusTitle()
        updateActionStates()
    }

    @discardableResult
    private func applyGatewayStatusFailure(_ error: Error) -> Bool {
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
        }
        return missingConfiguration
    }

    func updateQuotaStatus(title: String, detail: String?, progress: Double?) {
        providerUsageDashboardView?.updateQuotaStatus(title: title, detail: detail)
    }

    func updateProviderQuotaStatus(_ usages: [ProviderQuotaUsage]) {
        providerUsageDashboardView?.updateQuotaUsages(usages)
    }

    func updateTokenUsageStatus(title: String, detail: String?, progress: Double?) {
        providerUsageDashboardView?.updateTokenStatus(title: title, detail: detail)
    }

    func updateProviderTokenUsageStatus(_ usages: [ProviderTokenUsage]) {
        providerUsageDashboardView?.updateTokenUsages(usages)
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
        let operationID = String(UUID().uuidString.prefix(8))
        let command = diagnosticCommandDescription(arguments)
        let startedAt = Date()
        appendDiagnosticLog(
            "APP_OPERATION started id=\(operationID) command=\(command.isEmpty ? "<default>" : command)"
        )
        do {
            let output = try await runProcess(try gatewayExecutableURL().path, arguments)
            let durationMilliseconds = Int(Date().timeIntervalSince(startedAt) * 1_000)
            appendDiagnosticLog(
                """
                APP_OPERATION completed id=\(operationID) duration_ms=\(durationMilliseconds) command=\(command.isEmpty ? "<default>" : command)
                \(diagnosticOutputSummary(arguments: arguments, output: output))
                """
            )
            return output
        } catch {
            let durationMilliseconds = Int(Date().timeIntervalSince(startedAt) * 1_000)
            appendDiagnosticLog(
                """
                APP_OPERATION failed id=\(operationID) duration_ms=\(durationMilliseconds) command=\(command.isEmpty ? "<default>" : command)
                \(diagnosticErrorDescription(error))
                """
            )
            throw error
        }
    }

    func runGatewayStreaming(_ arguments: [String], onProgress: @escaping (String) -> Void) async throws -> String {
        let executable = try gatewayExecutableURL().path
        return try await runProcessStreaming(executable, arguments, onProgress: onProgress)
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
        let operationID = String(UUID().uuidString.prefix(8))
        let command = diagnosticCommandDescription(arguments)
        let startedAt = Date()
        appendDiagnosticLog(
            "APP_PROCESS started id=\(operationID) executable=\(executable) arguments=\(command)"
        )
        let diagnosticDirectory = stateDir()
        return try await withCheckedThrowingContinuation { continuation in
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
                    let result = try runProcessCollectingMergedOutput(
                        process,
                        outputPipe: outputPipe,
                        timeout: 600
                    )
                    let output = String(data: result.data, encoding: .utf8) ?? ""
                    let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
                    let durationMilliseconds = Int(Date().timeIntervalSince(startedAt) * 1_000)
                    if result.terminationStatus == 0 {
                        appendAppDiagnosticLog(
                            """
                            APP_PROCESS completed id=\(operationID) duration_ms=\(durationMilliseconds) exit=0 executable=\(executable) arguments=\(command) output_bytes=\(output.lengthOfBytes(using: .utf8))
                            """,
                            directory: diagnosticDirectory
                        )
                        continuation.resume(returning: trimmed)
                    } else {
                        appendAppDiagnosticLog(
                            """
                            APP_PROCESS failed id=\(operationID) duration_ms=\(durationMilliseconds) exit=\(result.terminationStatus) executable=\(executable) arguments=\(command) output_bytes=\(output.lengthOfBytes(using: .utf8))
                            """,
                            directory: diagnosticDirectory
                        )
                        continuation.resume(throwing: GatewayError.command(trimmed.isEmpty ? "exit \(result.terminationStatus)" : trimmed))
                    }
                } catch {
                    let durationMilliseconds = Int(Date().timeIntervalSince(startedAt) * 1_000)
                    appendAppDiagnosticLog(
                        """
                        APP_PROCESS launch_failed id=\(operationID) duration_ms=\(durationMilliseconds) executable=\(executable) arguments=\(command)
                        \(diagnosticErrorDescription(error))
                        """,
                        directory: diagnosticDirectory
                    )
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    func runProcessStreaming(_ executable: String, _ arguments: [String], onProgress: @escaping (String) -> Void) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                let process = Process()
                let outputPipe = Pipe()
                let errorPipe = Pipe()
                process.executableURL = URL(fileURLWithPath: executable)
                process.arguments = arguments
                process.standardOutput = outputPipe
                process.standardError = errorPipe
                let collector = StreamingProcessOutputCollector(
                    progressPrefix: "MIXIN_PROGRESS ",
                    onProgress: onProgress
                )
                do {
                    let result = try runProcessCollectingStreamingOutput(
                        process,
                        outputPipe: outputPipe,
                        errorPipe: errorPipe,
                        collector: collector
                    )
                    let text = String(decoding: result.data, as: UTF8.self)
                    if result.terminationStatus == 0 { continuation.resume(returning: text.trimmingCharacters(in: .whitespacesAndNewlines)) }
                    else { continuation.resume(throwing: GatewayError.command(text.isEmpty ? "exit \(result.terminationStatus)" : text)) }
                } catch { continuation.resume(throwing: error) }
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
        appendAppDiagnosticLog(message, directory: stateDir())
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
