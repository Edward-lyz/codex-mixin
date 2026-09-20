import Cocoa
import UniformTypeIdentifiers

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
                        try await bootstrapLaunchAgent()
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

    @objc func exportConfigBackup() {
        let panel = NSSavePanel()
        panel.title = "导出配置备份"
        panel.message = "备份使用 Base64 编码但未加密，包含 API Key、AWS 凭据和本地访问密钥，请妥善保管。"
        panel.nameFieldStringValue = "codex-mixin-config.b64"
        panel.allowedContentTypes = [UTType(filenameExtension: "b64") ?? .data]
        guard panel.runModal() == .OK, let destination = panel.url else { return }
        Task { @MainActor in
            do {
                _ = try await runGateway(["config", "export", destination.path])
                showAlert(
                    title: "配置备份已导出",
                    message: "Base64 备份已保存到 \(destination.path)，文件权限为仅当前用户可读写。Base64 不是加密，请勿公开分享。"
                )
            } catch {
                showAlert(title: "导出配置备份失败", message: String(describing: error))
            }
        }
    }

    @objc func importConfigBackup() {
        let panel = NSOpenPanel()
        panel.title = "导入配置备份"
        panel.message = "选择 Codex Mixin 导出的 Base64 配置备份。"
        panel.allowedContentTypes = [UTType(filenameExtension: "b64") ?? .data]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        guard panel.runModal() == .OK, let source = panel.url else { return }
        guard confirm(
            title: "导入配置备份？",
            message: "这会替换当前 Provider、模型选择、Fusion 和凭据配置，然后重启本地网关。"
        ) else { return }

        serviceBusy = true
        Task { @MainActor in
            var imported = false
            defer { serviceBusy = false }
            do {
                _ = try await runGateway(["config", "import", source.path])
                imported = true
                try await restartGatewayProcess()
                let status = try await waitForGatewayStatus()
                applyGatewayStatus(status)
                await refreshStatusNow()
                showAlert(
                    title: "配置备份已导入",
                    message: "配置已安全写入并应用，本地网关已重启。"
                )
            } catch {
                showAlert(
                    title: imported ? "配置已导入，但应用失败" : "导入配置备份失败",
                    message: String(describing: error)
                )
                await refreshStatusNow()
            }
        }
    }

    @objc func quit() {
        NSApp.terminate(nil)
    }

}
