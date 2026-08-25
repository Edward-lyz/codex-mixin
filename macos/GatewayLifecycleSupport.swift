import Cocoa

extension AppDelegate {
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

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if updateTerminationReady {
            timer?.invalidate()
            return .terminateNow
        }
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
