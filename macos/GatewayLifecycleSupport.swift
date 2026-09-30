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
                let currentStatus = try await runGateway(["status", "--json"])
                let snapshot = try decodeGatewayStatus(currentStatus)
                if snapshot.configured == false {
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
                if FileManager.default.fileExists(atPath: menuLaunchAgentPath().path) {
                    try installMenuLaunchAgent()
                }
                let status = try await runGateway(["service", "ensure", "--json"])
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
        try await runGateway(["service", "ensure", "--json"])
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if updateTerminationReady {
            refreshTimer?.stop()
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
                _ = try await runGateway(["service", "stop", "--managed", "--json"])
                refreshTimer?.stop()
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
