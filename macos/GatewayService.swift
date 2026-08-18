import Cocoa

private struct GatewayHealthResponse: Decodable {
    let ok: Bool
    let providerReadiness: String?

    enum CodingKeys: String, CodingKey {
        case ok
        case providerReadiness = "provider_readiness"
    }
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
                loadCachedProviderQuota()
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
                await refreshStatusNow()
                Task { @MainActor in
                    do {
                        _ = try await runGateway(["refresh-codex-catalog"])
                    } catch {
                        showAlert(title: "刷新 Codex 模型失败", message: String(describing: error))
                    }
                }
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
            for provider in response.providers where provider.needsInitialModelDiscovery {
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

    @MainActor
    func performStatusRefresh(
        isCurrent: @escaping StatusRefreshCoordinator.IsCurrent
    ) async {
        let scope = pendingStatusRefreshScope ?? .full
        pendingStatusRefreshScope = nil
        if scope == .health {
            do {
                let health = try await checkGatewayHealth()
                guard isCurrent() else { return }
                applyHealthyGatewaySnapshot(health)
            } catch {
                do {
                    let status = try await runGateway(["status"])
                    guard isCurrent() else { return }
                    applyGatewayStatus(status)
                } catch {
                    guard isCurrent() else { return }
                    _ = applyGatewayStatusFailure(error)
                }
            }
            return
        }

        do {
            let status = try await runGateway(["status"])
            guard isCurrent() else { return }
            applyGatewayStatus(status)
        } catch {
            guard isCurrent() else { return }
            if applyGatewayStatusFailure(error) {
                return
            }
        }
        guard scope == .full || quotaRefreshPolicy.isDue() else { return }
        quotaRefreshPolicy.markAttempt()
        // Quota pages can involve several remote providers and browser-backed
        // dashboards. Start both subprocesses together so a slow quota probe
        // cannot hide the local token history on a fresh app launch.
        let providersTask = Task { try await runGateway(["providers", "list", "--json"]) }
        let quotaTask = Task { try await runGateway(["quota", "--json"]) }
        let usageTask = Task { try await runGateway(["usage", "--json"]) }
        do {
            let providerList = try decodeProviderList(await providersTask.value)
            guard isCurrent() else { return }
            let dashboardProviders = providerList.providers.map {
                ProviderDashboardProvider(
                    id: $0.id,
                    displayName: $0.displayName,
                    isEnabled: $0.enabled,
                    websiteURL: $0.websiteURL
                )
            }
            await MainActor.run {
                providerUsageDashboardView?.updateConfiguredProviders(dashboardProviders)
            }
            Task { [weak self] in
                guard let self else { return }
                for provider in dashboardProviders {
                    guard let websiteURL = provider.websiteURL else { continue }
                    do {
                        if try await refreshProviderLogoIfNeeded(
                            providerID: provider.id,
                            websiteURL: websiteURL
                        ) {
                            await MainActor.run {
                                self.providerUsageDashboardView?.refreshProviderIcons()
                            }
                        }
                    } catch {
                        appendAppDiagnosticLog(
                            "provider icon refresh failed for \(provider.id): \(diagnosticErrorDescription(error))",
                            directory: self.stateDir()
                        )
                    }
                }
            }
        } catch {
            guard isCurrent() else { return }
            updateQuotaStatus(
                title: "Provider 列表：不可用",
                detail: localizedErrorDescription(error),
                progress: nil
            )
        }
        do {
            let usage = try await usageTask.value
            guard isCurrent() else { return }
            updateProviderTokenUsageStatus(try parseProviderTokenUsage(usage))
        } catch {
            guard isCurrent() else { return }
            updateTokenUsageStatus(
                title: "Token 使用：不可用",
                detail: localizedErrorDescription(error),
                progress: nil
            )
        }
        do {
            let quota = try await quotaTask.value
            guard isCurrent() else { return }
            saveProviderQuotaCache(quota)
            updateProviderQuotaStatus(try parseProviderQuotaUsage(quota))
        } catch {
            guard isCurrent() else { return }
            updateQuotaStatus(
                title: "Provider 额度：不可用",
                detail: localizedErrorDescription(error),
                progress: nil
            )
        }
    }

    private func checkGatewayHealth() async throws -> GatewayHealthResponse {
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
        guard let httpResponse = response as? HTTPURLResponse,
              (200 ... 299).contains(httpResponse.statusCode)
        else {
            throw GatewayError.command("gateway health check failed")
        }
        let health = try JSONDecoder().decode(GatewayHealthResponse.self, from: data)
        guard health.ok else {
            throw GatewayError.command("gateway health check failed")
        }
        return health
    }

    private func applyHealthyGatewaySnapshot(_ health: GatewayHealthResponse) {
        isRunning = true
        if health.providerReadiness == "degraded" {
            serviceStatus = "本地网关运行中 · Provider 降级"
            if providerStatusDetail == nil {
                providerStatusDetail = "Provider 配置或模型缓存需要处理"
            }
        } else if health.providerReadiness == "disabled" {
            providerStatusDetail = nil
            serviceStatus = "本地网关运行中 · 无启用 Provider"
        } else {
            providerStatusDetail = nil
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

    func loadCachedProviderQuota() {
        let url = stateDir().appendingPathComponent("quota-cache.json")
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        do {
            let rawJSON = try String(contentsOf: url, encoding: .utf8)
            updateProviderQuotaStatus(try parseProviderQuotaUsage(rawJSON))
        } catch {
            appendDiagnosticLog(
                "Failed to load cached provider quota: \(localizedErrorDescription(error))"
            )
        }
    }

    private func saveProviderQuotaCache(_ rawJSON: String) {
        let url = stateDir().appendingPathComponent("quota-cache.json")
        do {
            try FileManager.default.createDirectory(
                at: stateDir(),
                withIntermediateDirectories: true
            )
            try rawJSON.write(to: url, atomically: true, encoding: .utf8)
        } catch {
            appendDiagnosticLog(
                "Failed to save provider quota cache: \(localizedErrorDescription(error))"
            )
        }
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

}
